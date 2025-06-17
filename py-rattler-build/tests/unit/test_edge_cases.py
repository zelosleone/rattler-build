from pathlib import Path

import pytest
import rattler_build
import shutil


def test_build_with_invalid_recipe(tmp_path: Path) -> None:
    recipe_path = tmp_path.joinpath("invalid_recipe.yaml")
    recipe_path.write_text("invalid: yaml: content:")
    output_dir = tmp_path.joinpath("output")
    
    with pytest.raises(RuntimeError):
        rattler_build.build_recipes(
            [recipe_path],
            output_dir=output_dir
        )


def test_build_with_invalid_platform(tmp_path: Path) -> None:
    recipe_path = tmp_path.joinpath("dummy_recipe.yaml")
    recipe_path.write_text("""
recipe:
  name: test
  version: 1.0.0
""")
    
    with pytest.raises(RuntimeError, match="is not a known platform"):
        rattler_build.build_recipes(
            [recipe_path],
            target_platform="invalid-platform-123"
        )


def test_upload_conda_forge_with_invalid_urls() -> None:
    with pytest.raises(RuntimeError, match="relative URL without a base"):
        rattler_build.upload_packages_to_conda_forge(
            ["package.tar.bz2"],
            "staging_token",
            "feedstock",
            "feedstock_token",
            anaconda_url="not-a-url"
        )
    
    with pytest.raises(RuntimeError, match="relative URL without a base"):
        rattler_build.upload_packages_to_conda_forge(
            ["package.tar.bz2"],
            "staging_token",
            "feedstock",
            "feedstock_token",
            validation_endpoint="not-a-url"
        )


def test_build_continue_on_failure(tmp_path: Path, recipes_dir: Path) -> None:
    valid_recipe = tmp_path.joinpath("valid", "recipe.yaml")
    valid_recipe.parent.mkdir(parents=True)
    shutil.copy(recipes_dir.joinpath("dummy", "recipe.yaml"), valid_recipe)
    
    invalid_recipe = tmp_path.joinpath("invalid", "recipe.yaml")
    invalid_recipe.parent.mkdir(parents=True)
    invalid_recipe.write_text("invalid: yaml: content:")
    
    output_dir = tmp_path.joinpath("output")
    
    with pytest.raises(RuntimeError):
        rattler_build.build_recipes(
            [invalid_recipe, valid_recipe],
            output_dir=output_dir,
            continue_on_failure=False
        )
    
    output_dir2 = tmp_path.joinpath("output2")
    with pytest.raises(RuntimeError):
        rattler_build.build_recipes(
            [invalid_recipe, valid_recipe],
            output_dir=output_dir2,
            continue_on_failure=True
        )