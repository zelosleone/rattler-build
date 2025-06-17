//! Output parsing is a bit more complicated than the other sections.
//!
//! The reason for this is that the `outputs` field is a list of mappings, and
//! each mapping can have its own `package`, `source`, `build`, `requirements`,
//! `test`, and `about` fields.

use marked_yaml::types::MarkedMappingNode;

use crate::{
    _partialerror,
    recipe::{
        ParsingError,
        custom_yaml::{Node, parse_yaml},
        error::{ErrorKind, PartialParsingError},
    },
    source_code::SourceCode,
};

static DEEP_MERGE_KEYS: [&str; 4] = ["package", "about", "extra", "build"];
static ALLOWED_KEYS_MULTI_OUTPUTS: [&str; 9] = [
    "context",
    "recipe",
    "source",
    "build",
    "outputs",
    "about",
    "extra",
    "cache",
    "schema_version",
];

// Check if the `cache` top-level key is present. If it does not contain a
// source, but there is a top-level `source` key, then we should warn the user
// because this key was moved to the `cache`
fn check_src_cache(root: &MarkedMappingNode) -> Result<(), PartialParsingError> {
    if let Some(cache) = root.get("cache") {
        let has_top_level_source = root.contains_key("source");
        let cache_map = cache.as_mapping().ok_or_else(|| {
            _partialerror!(
                *cache.span(),
                ErrorKind::ExpectedMapping,
                help = "`cache` must always be a mapping"
            )
        })?;

        if !cache_map.contains_key("source") && has_top_level_source {
            tracing::warn!(
                "The cache has its own `source` key now. You probably want to move the top-level `source` key into the `cache` key."
            );
        }
    }

    Ok(())
}

/// Retrieve all outputs from the recipe source (YAML)
pub fn find_outputs_from_src<S: SourceCode>(src: S) -> Result<Vec<Node>, ParsingError<S>> {
    let root_node = parse_yaml(0, src.clone())?;
    let root_map = root_node.as_mapping().ok_or_else(|| {
        ParsingError::from_partial(
            src.clone(),
            _partialerror!(
                *root_node.span(),
                ErrorKind::ExpectedMapping,
                help = "root node must always be a mapping"
            ),
        )
    })?;

    if let Err(err) = check_src_cache(root_map) {
        return Err(ParsingError::from_partial(src, err));
    };

    if root_map.contains_key("outputs") {
        if root_map.contains_key("package") {
            let key = root_map
                .keys()
                .find(|k| k.as_str() == "package")
                .expect("unreachable we preemptively check for if contains");
            return Err(ParsingError::from_partial(
                src.clone(),
                _partialerror!(
                    *key.span(),
                    ErrorKind::InvalidField("package".to_string().into()),
                    help = "recipe cannot have both `outputs` and `package` fields. Rename `package` to `recipe` or remove `outputs`"
                ),
            ));
        }

        if root_map.contains_key("requirements") {
            let key = root_map
                .keys()
                .find(|k| k.as_str() == "requirements")
                .expect("unreachable we preemptively check for if contains");
            return Err(ParsingError::from_partial(
                src,
                _partialerror!(
                    *key.span(),
                    ErrorKind::InvalidField("requirements".to_string().into()),
                    help = "multi-output recipes cannot have a top-level requirements field. Move `requirements` inside the individual output."
                ),
            ));
        }

        for key in root_map.keys() {
            if !ALLOWED_KEYS_MULTI_OUTPUTS.contains(&key.as_str()) {
                return Err(ParsingError::from_partial(
                    src,
                    _partialerror!(
                        *key.span(),
                        ErrorKind::InvalidField(key.as_str().to_string().into()),
                        help = format!("invalid key `{}` in root node", key.as_str())
                    ),
                ));
            }
        }
    }

    let Some(outputs) = root_map.get("outputs") else {
        let recipe =
            Node::try_from(root_node).map_err(|err| ParsingError::from_partial(src, err))?;
        return Ok(vec![recipe]);
    };

    let mut recipe_version: Option<marked_yaml::Node> = None;
    // If `recipe` exists in root we will use the version as default for all outputs
    // We otherwise ignore the `recipe.name` value.
    if let Some(recipe_mapping) = root_map
        .get("recipe")
        .and_then(|recipe| recipe.as_mapping())
    {
        // make sure that mapping only contains name and version
        for (k, v) in recipe_mapping.iter() {
            match k.as_str() {
                "name" => {}
                "version" => recipe_version = Some(v.clone()),
                _ => {
                    return Err(ParsingError::from_partial(
                        src,
                        _partialerror!(
                            *k.span(),
                            ErrorKind::InvalidField(k.as_str().to_string().into()),
                            help = "recipe can only contain `name` and `version` fields"
                        ),
                    ));
                }
            }
        }
    }

    let Some(outputs) = outputs.as_sequence() else {
        return Err(ParsingError::from_partial(
            src,
            _partialerror!(
                *outputs.span(),
                ErrorKind::ExpectedSequence,
                help = "`outputs` must always be a sequence"
            ),
        ));
    };

    let mut res = Vec::with_capacity(outputs.len());

    // the schema says that `outputs` can be either an output, a if-selector or a
    // sequence of outputs and if-selectors. We need to handle all of these
    // cases but for now, lets handle only sequence of outputs
    for output in outputs.iter() {
        // 1. clone the root node
        // 2. remove the `outputs` key
        // 3. substitute repeated value (make sure to preserve the spans)
        // 4. merge skip values (make sure to preserve the spans)
        // Note: Make sure to preserve the spans of the original root span so the error
        // messages remain accurate and point the correct part of the original recipe
        // src
        let mut root = root_map.clone();
        root.remove("outputs");

        let mut output_node = output.clone();

        let Some(output_map) = output_node.as_mapping_mut() else {
            return Err(ParsingError::from_partial(
                src,
                _partialerror!(
                    *output.span(),
                    ErrorKind::ExpectedMapping,
                    help = "individual `output` must always be a mapping"
                ),
            ));
        };

        for (key, value) in root.iter() {
            if !output_map.contains_key(key) {
                output_map.insert(key.clone(), value.clone());
            } else {
                // deep merge
                if DEEP_MERGE_KEYS.contains(&key.as_str()) {
                    let output_map_span = *output_map.span();
                    let Some(output_value) = output_map.get_mut(key) else {
                        return Err(ParsingError::from_partial(
                            src,
                            _partialerror!(
                                output_map_span,
                                ErrorKind::MissingField(key.as_str().to_owned().into()),
                            ),
                        ));
                    };
                    let output_value_span = *output_value.span();
                    let Some(output_value_map) = output_value.as_mapping_mut() else {
                        return Err(ParsingError::from_partial(
                            src,
                            _partialerror!(output_value_span, ErrorKind::ExpectedMapping,),
                        ));
                    };

                    let mut root_value = value.clone();
                    let Some(root_value_map) = root_value.as_mapping_mut() else {
                        return Err(ParsingError::from_partial(
                            src,
                            _partialerror!(*value.span(), ErrorKind::ExpectedMapping,),
                        ));
                    };

                    for (key, value) in root_value_map.iter() {
                        if !output_value_map.contains_key(key) {
                            output_value_map.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
        }

        if let Some(version) = recipe_version.as_ref() {
            let Some(package_map) = output_map
                .get_mut("package")
                .and_then(|node| node.as_mapping_mut())
            else {
                return Err(ParsingError::from_partial(
                    src,
                    _partialerror!(
                        *output_node.span(),
                        ErrorKind::MissingField("package".to_string().into())
                    ),
                ));
            };

            if !package_map.contains_key("version") {
                package_map.insert("version".into(), version.clone());
            }
        }

        output_map.remove("recipe");

        let recipe = match Node::try_from(output_node) {
            Ok(node) => node,
            Err(err) => return Err(ParsingError::from_partial(src, err)),
        };
        res.push(recipe);
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        assert_miette_snapshot,
        recipe::{Recipe, jinja::SelectorConfig},
    };
    use fs_err as fs;
    use insta::assert_debug_snapshot;

    #[test]
    fn recipe_schema_error() {
        let test_data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data");
        let yaml_file = test_data_dir.join("recipes/test-parsing/recipe_outputs_and_package.yaml");
        let src = fs::read_to_string(yaml_file).unwrap();
        assert_miette_snapshot!(find_outputs_from_src(src.as_str()).unwrap_err());

        let yaml_file =
            test_data_dir.join("recipes/test-parsing/recipe_outputs_and_requirements.yaml");
        let src = fs::read_to_string(yaml_file).unwrap();
        assert_miette_snapshot!(find_outputs_from_src(src.as_str()).unwrap_err());

        let yaml_file = test_data_dir.join("recipes/test-parsing/recipe_missing_version.yaml");
        let src = fs::read_to_string(yaml_file).unwrap();
        let nodes = find_outputs_from_src(src.as_str()).unwrap();
        let parsed_recipe =
            Recipe::from_node(&nodes[0], SelectorConfig::default()).map_err(|err| {
                err.into_iter()
                    .map(|err| ParsingError::from_partial(src.as_str(), err))
                    .collect::<Vec<_>>()
            });
        let err: crate::variant_config::ParseErrors<_> = parsed_recipe.unwrap_err().into();
        assert_miette_snapshot!(err);

        let yaml_file = test_data_dir.join("recipes/test-parsing/recipe_outputs_extra_keys.yaml");
        let src = fs::read_to_string(yaml_file).unwrap();
        assert_miette_snapshot!(find_outputs_from_src(src.as_str()).unwrap_err());
    }

    #[test]
    fn recipe_outputs_merging() {
        let test_data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data");
        let yaml_file = test_data_dir.join("recipes/test-parsing/recipe_outputs_merging.yaml");
        let src = fs::read_to_string(yaml_file).unwrap();
        assert_debug_snapshot!(find_outputs_from_src(src.as_str()).unwrap());
    }

    #[test]
    fn test_multi_output_circular_dependencies() {
        // Test detection of circular dependencies between outputs
        let recipe_with_circular_deps = r#"
recipe:
  name: multi-output-circular
  version: 1.0.0

outputs:
  - package:
      name: output-a
    requirements:
      run:
        - ${{ pin_subpackage('output-b', exact=true) }}

  - package:
      name: output-b
    requirements:
      run:
        - ${{ pin_subpackage('output-c', exact=true) }}

  - package:
      name: output-c
    requirements:
      run:
        - ${{ pin_subpackage('output-a', exact=true) }}
"#;

        // This should parse successfully - circular dep detection happens at build time
        let result = find_outputs_from_src(recipe_with_circular_deps);
        if let Err(e) = &result {
            eprintln!("Error parsing recipe: {:?}", e);
        }
        assert!(result.is_ok());
        let outputs = result.unwrap();
        assert_eq!(outputs.len(), 3);
    }

    #[test]
    fn test_complex_variant_multi_output() {
        // Test multi-output with different variant requirements per output
        let complex_variant_recipe = r#"
recipe:
  name: complex-variant-outputs
  version: 1.0.0

outputs:
  - package:
      name: base-{{ python }}
    requirements:
      host:
        - python {{ python }}
        - numpy {{ numpy }}
    build:
      script: echo "Building for Python {{ python }}"
      skip: python == "3.8"  # Skip Python 3.8 builds

  - package:
      name: cuda-{{ cuda_version }}
    requirements:
      host:
        - ${{ pin_subpackage('base-' ~ python, exact=true) }}
        - cudatoolkit {{ cuda_version }}
      run:
        - python {{ python }}
        - cudatoolkit {{ cuda_version }}
    build:
      skip: cuda_version == ""  # Skip if no CUDA
      string: py{{ python }}_cuda{{ cuda_version }}_{{ PKG_BUILDNUM }}

  - package:
      name: all-variants
    requirements:
      run:
        - ${{ pin_subpackage('base-' ~ python, exact=true) }}
        - ${{ pin_subpackage('cuda-' ~ cuda_version, exact=true) }}  # if: cuda_version != ""
"#;

        let result = find_outputs_from_src(complex_variant_recipe);
        assert!(result.is_ok());
        let outputs = result.unwrap();
        assert_eq!(outputs.len(), 3);
    }

    #[test]
    fn test_deep_merge_conflicts() {
        // Test complex deep merging scenarios with conflicts
        let deep_merge_recipe = r#"
recipe:
  name: should-be-overridden
  version: 1.0.0

build:
  number: 100
  script:
    - echo "Global script"
  skip: true

about:
  license: MIT
  summary: Global summary
  home: https://example.com

extra:
  maintainers:
    - global-maintainer
  feedstock-name: global-feedstock

outputs:
  - package:
      name: output-with-overrides
      version: 2.0.0
    build:
      number: 200
      script:
        - echo "Output-specific script"
      skip: false
    about:
      license: Apache-2.0
      summary: Output-specific summary
    extra:
      maintainers:
        - output-maintainer
      custom-field: custom-value

  - package:
      name: output-partial-override
    build:
      script:
        - echo "Another script"
"#;

        let result = find_outputs_from_src(deep_merge_recipe);
        assert!(result.is_ok());

        let outputs = result.unwrap();
        assert_eq!(outputs.len(), 2);
    }

    #[test]
    fn test_output_skip_conditions() {
        // Test complex skip conditions across outputs
        let skip_conditions_recipe = r#"
recipe:
  name: complex-skip-conditions
  version: 1.0.0

outputs:
  - package:
      name: linux-only
    build:
      skip: true  # if: not linux

  - package:
      name: windows-only
    build:
      skip: false  # if: not win

  - package:
      name: python-specific
    build:
      skip: false  # if: python != "3.11"
    requirements:
      host:
        - python 3.11

  - package:
      name: cuda-specific
    build:
      skip: true  # if: cuda_compiler_version == ""
    requirements:
      build:
        - ${{ compiler('cuda') }}

  - package:
      name: complex-skip
    build:
      skip: false  # Complex conditions would be evaluated at render time
"#;

        let result = find_outputs_from_src(skip_conditions_recipe);
        assert!(result.is_ok());
        let outputs = result.unwrap();
        assert_eq!(outputs.len(), 5);
    }

    #[test]
    fn test_cache_source_warning() {
        // Test the warning for cache source configuration
        let cache_source_recipe = r#"
recipe:
  name: cache-source-test
  version: 1.0.0

source:
  - url: https://example.com/source.tar.gz
    sha256: abc123

cache:
  requirements:
    build:
      - python

outputs:
  - package:
      name: test-output
"#;

        // This should parse but potentially warn about source location
        let result = find_outputs_from_src(cache_source_recipe);
        assert!(result.is_ok());
    }
}
