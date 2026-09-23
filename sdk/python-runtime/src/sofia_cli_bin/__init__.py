import os
from pathlib import Path

PACKAGE_NAME = "openai-sofia-cli-bin"
PACKAGE_METADATA_FILENAME = "sofia-package.json"


def bundled_package_dir() -> Path:
    path = Path(__file__).resolve().parent
    metadata_path = path / PACKAGE_METADATA_FILENAME
    if not metadata_path.is_file():
        raise FileNotFoundError(
            f"{PACKAGE_NAME} is installed but missing its package metadata at {metadata_path}"
        )
    return path


def bundled_codex_path() -> Path:
    exe = "sofia.exe" if os.name == "nt" else "sofia"
    path = bundled_package_dir() / "bin" / exe
    if not path.is_file():
        raise FileNotFoundError(
            f"{PACKAGE_NAME} is installed but missing its packaged sofia binary at {path}"
        )
    return path


def bundled_path_dir() -> Path | None:
    path = bundled_package_dir() / "sofia-path"
    return path if path.is_dir() else None


__all__ = [
    "PACKAGE_NAME",
    "bundled_codex_path",
    "bundled_package_dir",
    "bundled_path_dir",
]
