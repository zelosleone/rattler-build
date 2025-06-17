from pathlib import Path
from typing import List, Union

import pytest
import rattler_build
import shutil


def test_build_with_multiple_options(tmp_path: Path, recipes_dir: Path) -> None:
    recipe_name = "recipe.yaml"
    recipe_path = tmp_path.joinpath(recipe_name)
    shutil.copy(recipes_dir.joinpath("dummy", recipe_name), recipe_path)
    output_dir = tmp_path.joinpath("output")
    
    rattler_build.build_recipes(
        [recipe_path],
        output_dir=output_dir,
        with_solve=True,
        no_build_id=True,
        package_format="tar-bz2",
        compression_threads=2,
        channel=["conda-forge"],
        channel_priority="strict",
        no_include_recipe=False
    )
    
    assert output_dir.joinpath("noarch").is_dir()


def test_build_with_variant_config(tmp_path: Path, recipes_dir: Path) -> None:
    recipe_name = "recipe.yaml"
    recipe_path = tmp_path.joinpath(recipe_name)
    shutil.copy(recipes_dir.joinpath("dummy", recipe_name), recipe_path)
    
    variant_config = tmp_path.joinpath("variants.yaml")
    variant_config.write_text("""
python:
  - "3.9"
  - "3.10"
""")
    
    output_dir = tmp_path.joinpath("output")
    
    rattler_build.build_recipes(
        [recipe_path],
        output_dir=output_dir,
        variant_config=[str(variant_config)],
        ignore_recipe_variants=False
    )
    
    assert output_dir.joinpath("noarch").is_dir()


def test_test_package_with_options(tmp_path: Path, recipes_dir: Path) -> None:
    recipe_name = "recipe.yaml"
    recipe_path = tmp_path.joinpath(recipe_name)
    shutil.copy(recipes_dir.joinpath("dummy", recipe_name), recipe_path)
    output_dir = tmp_path.joinpath("output")
    
    rattler_build.build_recipes([recipe_path], output_dir=output_dir, test="skip")
    
    conda_files = list(output_dir.glob("**/*.conda"))
    if not conda_files:
        conda_files = list(output_dir.glob("**/*.tar.bz2"))
    
    assert len(conda_files) > 0
    
    rattler_build.test_package(
        conda_files[0],
        channel=["conda-forge"],
        compression_threads=2,
        channel_priority="strict"
    )


def test_build_multiple_recipes(tmp_path: Path) -> None:
    recipes: List[Union[str, Path]] = []
    for i in range(2):
        recipe_dir = tmp_path.joinpath(f"recipe_{i}")
        recipe_dir.mkdir()
        recipe_path = recipe_dir.joinpath("recipe.yaml")
        
        content = f"""
recipe:
  name: multi-test-{i}
  version: 1.0.{i}

outputs:
  - package:
      name: multi-test-{i}
      version: 1.0.{i}

    build:
      script:
        - mkdir -p $PREFIX/bin
        - echo "echo multi-test-{i}" > $PREFIX/bin/multi-test-{i}
        - chmod +x $PREFIX/bin/multi-test-{i} || true
"""
        recipe_path.write_text(content)
        recipes.append(recipe_path)
    
    output_dir = tmp_path.joinpath("output")
    
    rattler_build.build_recipes(
        recipes,
        output_dir=output_dir
    )
    
    assert output_dir.joinpath("noarch").is_dir()

