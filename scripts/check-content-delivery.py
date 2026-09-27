#!/usr/bin/env python3
"""Opt-in production HTTPS check of one locally pinned game object."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import urllib.parse
import urllib.request


def validate_url(url):
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username
            or parsed.password or parsed.query or parsed.fragment):
        raise ValueError("Content URLs must use HTTPS without credentials, query, or fragment.")
    return url


class HttpsRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        validate_url(new_url)
        return super().redirect_request(request, response, code, message, headers, new_url)


def pinned_objects(repository):
    delivery = json.loads((repository / "manifests/game-delivery.json").read_text())
    patches = json.loads((repository / "manifests/patches-1.23b.json").read_text())
    objects = {"patches/1.23b/" + item["runtimePath"]:
               (item["size"], item["sha256"]) for item in patches["files"]}
    for archive in (delivery.get("base") or {}).get("archives", []):
        item = archive["object"]
        if item["object_key"] in objects:
            raise ValueError("Duplicate object key in the local manifest.")
        objects[item["object_key"]] = (item["length"], item["sha256"])
    return delivery, objects


def check(opener, url, length, digest):
    def request(method="GET", extra=None):
        headers = {"Accept-Encoding": "identity", "User-Agent": "BahamutLauncher-ContentCheck/1"}
        headers.update(extra or {})
        response = opener.open(urllib.request.Request(url, headers=headers, method=method), timeout=30)
        if response.headers.get("Content-Encoding", "identity").lower() != "identity":
            response.close()
            raise ValueError("Server returned a transformed representation.")
        return response

    with request("HEAD") as response:
        if response.status != 200 or response.headers.get("Content-Length") != str(length):
            raise ValueError("HEAD status or Content-Length differs from the local pin.")
        cache = {name: response.headers.get(name) for name in
                 ("Cache-Control", "CF-Cache-Status", "Age", "Content-Type")}
    with request(extra={"Range": "bytes=0-0"}) as response:
        if (response.status != 206
                or response.headers.get("Content-Range") != f"bytes 0-0/{length}"
                or len(response.read(2)) != 1):
            raise ValueError("Production byte-range behavior did not match the requested byte.")
    actual = hashlib.sha256()
    count = 0
    with request() as response:
        if response.status != 200 or response.headers.get("Content-Length") != str(length):
            raise ValueError("Full download status or length differs from the local pin.")
        while chunk := response.read(min(1024 * 1024, length - count + 1)):
            count += len(chunk)
            if count > length:
                raise ValueError("Object exceeded its pinned length.")
            actual.update(chunk)
    if count != length or actual.hexdigest() != digest.lower():
        raise ValueError("Downloaded bytes failed the pinned length or SHA-256 check.")
    return {"length": count, "sha256": actual.hexdigest(), "headers": cache}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--content-root", help="Defaults to the checked-in delivery host.")
    parser.add_argument("--object-key", required=True, help="Exact key from the checked-in manifests.")
    args = parser.parse_args()
    try:
        delivery, objects = pinned_objects(Path(__file__).resolve().parent.parent)
        if args.object_key not in objects:
            raise ValueError("Object key is absent from the checked-in manifests.")
        root = args.content_root or delivery.get("content_root")
        if not root:
            raise ValueError("Set --content-root to the owner-controlled production HTTPS host.")
        validate_url(root)
        length, digest = objects[args.object_key]
        if length <= 0 or not re.fullmatch(r"[a-fA-F0-9]{64}", digest):
            raise ValueError("Invalid local object identity.")
        url = validate_url(root.rstrip("/") + "/" + urllib.parse.quote(args.object_key, safe="/"))
        result = check(urllib.request.build_opener(HttpsRedirect()), url, length, digest)
        print(json.dumps({"object_key": args.object_key, **result}, indent=2))
        return 0
    except (OSError, ValueError, KeyError) as error:
        print(f"check-content-delivery: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
