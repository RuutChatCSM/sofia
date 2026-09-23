#!/usr/bin/env python3
"""Publish a locally-built Sofia release directory to an S3-compatible bucket.

Unlike ``.github/scripts/publish_r2_release.py`` (which mirrors a GitHub
Release), this uploads the assets in a local directory directly, then writes
the same installer-facing ``release.json`` metadata and channel aliases under
``sofia/`` in the bucket.

Configuration comes from the environment:
  SOFIA_R2_BUCKET, SOFIA_R2_PUBLIC_BASE_URL, AWS_ENDPOINT_URL, AWS_REGION,
  AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any
from urllib.parse import quote

PREFIX = "sofia"
INSTALLER_NAMES = ("install.sh", "install.ps1")
RELEASE_METADATA_NAME = "release.json"
MAX_WORKERS = 3


class PublishError(RuntimeError):
    pass


def run(args: list[str]) -> str:
    result = subprocess.run(
        args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    if result.returncode != 0:
        raise PublishError(
            (result.stderr or result.stdout or "").strip() or f"{args[0]} failed"
        )
    return result.stdout


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def s3_uri(bucket: str, key: str) -> str:
    return f"s3://{bucket}/{key}"


def put_object(
    bucket: str, endpoint: str, key: str, path: Path, sha256: str, content_type: str
) -> None:
    args = [
        "aws",
        "s3",
        "cp",
        str(path),
        s3_uri(bucket, key),
        "--metadata",
        f"sha256={sha256}",
        "--content-type",
        content_type,
        "--endpoint-url",
        endpoint,
    ]
    run(args)


def head_object(bucket: str, endpoint: str, key: str) -> dict[str, Any] | None:
    result = subprocess.run(
        [
            "aws",
            "s3api",
            "head-object",
            "--bucket",
            bucket,
            "--key",
            key,
            "--endpoint-url",
            endpoint,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if result.returncode != 0:
        if (
            "404" in result.stderr
            or "Not Found" in result.stderr
            or "NoSuchKey" in result.stderr
        ):
            return None
        raise PublishError((result.stderr or result.stdout or "").strip())
    return json.loads(result.stdout)


def upload_if_needed(
    bucket: str, endpoint: str, key: str, path: Path, content_type: str
) -> dict[str, Any]:
    sha256 = sha256_file(path)
    existing = head_object(bucket, endpoint, key)
    if (
        existing is not None
        and existing.get("ContentLength") == path.stat().st_size
        and (existing.get("Metadata") or {}).get("sha256") == sha256
    ):
        print(f"up-to-date s3://{bucket}/{key}", file=sys.stderr)
    else:
        put_object(bucket, endpoint, key, path, sha256, content_type)
        print(
            f"uploaded s3://{bucket}/{key} size={path.stat().st_size} sha256={sha256}",
            file=sys.stderr,
        )
    return {"name": path.name, "sha256": sha256, "size": path.stat().st_size}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist-dir", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--make-latest", choices=("true", "false"), default="true")
    parser.add_argument("--prerelease", choices=("true", "false"), default="false")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        bucket = os.environ.get("SOFIA_R2_BUCKET")
        endpoint = os.environ.get("AWS_ENDPOINT_URL")
        public_base = os.environ.get("SOFIA_R2_PUBLIC_BASE_URL")
        for name, value in (
            ("SOFIA_R2_BUCKET", bucket),
            ("AWS_ENDPOINT_URL", endpoint),
            ("SOFIA_R2_PUBLIC_BASE_URL", public_base),
        ):
            if not value:
                raise PublishError(f"{name} is required")

        version = args.tag.removeprefix("rust-v")
        dist = Path(args.dist_dir)
        if not dist.is_dir():
            raise PublishError(f"dist dir not found: {dist}")

        assets = sorted(path for path in dist.rglob("*") if path.is_file())
        if not assets:
            raise PublishError(f"no assets found under {dist}")

        results: dict[str, dict[str, Any]] = {}
        with ThreadPoolExecutor(max_workers=MAX_WORKERS) as pool:
            futures = {
                pool.submit(
                    upload_if_needed,
                    bucket,
                    endpoint,
                    f"{PREFIX}/releases/{version}/{path.name}",
                    path,
                    "application/octet-stream",
                ): path
                for path in assets
            }
            for future in as_completed(futures):
                path = futures[future]
                results[path.name] = future.result()

        metadata_assets = [
            {
                "name": path.name,
                "digest": f"sha256:{results[path.name]['sha256']}",
                "browser_download_url": (
                    f"{public_base}/releases/{version}/{quote(path.name, safe='')}"
                ),
            }
            for path in assets
        ]

        with tempfile.TemporaryDirectory() as temp_dir:
            metadata_path = Path(temp_dir) / RELEASE_METADATA_NAME
            metadata_path.write_text(
                json.dumps({"assets": metadata_assets, "tag_name": args.tag}, indent=2)
                + "\n",
                encoding="utf-8",
            )
            upload_if_needed(
                bucket,
                endpoint,
                f"{PREFIX}/releases/{version}/{RELEASE_METADATA_NAME}",
                metadata_path,
                "application/json",
            )

            for name in INSTALLER_NAMES:
                installer = dist / name
                if installer.is_file():
                    upload_if_needed(
                        bucket,
                        endpoint,
                        f"{PREFIX}/{name}",
                        installer,
                        "text/plain; charset=utf-8",
                    )

            channels = []
            if args.make_latest == "true":
                channels.append("latest")
            if args.prerelease == "true":
                channels.append("prerelease")
            for channel in channels:
                upload_if_needed(
                    bucket,
                    endpoint,
                    f"{PREFIX}/channels/{channel}",
                    metadata_path,
                    "application/json",
                )

        print(
            json.dumps(
                {"assetCount": len(assets), "tag": args.tag, "version": version},
                sort_keys=True,
            )
        )
        return 0
    except PublishError as error:
        print(f"publish failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
