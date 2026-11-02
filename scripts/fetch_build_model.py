"""Fetch an explicitly requested CI model into RUNNER_TEMP, pinned by SHA-256."""

import hashlib
import os
from pathlib import Path
import re
import tempfile
import urllib.parse
import urllib.request


class HTTPSOnlyRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        if urllib.parse.urlsplit(new_url).scheme != "https":
            raise ValueError("model redirects must use HTTPS")
        return super().redirect_request(request, response, code, message, headers, new_url)


def fetch():
    url = os.environ.get("MINI_MODEL_URL", "")
    expected = os.environ.get("MINI_MODEL_SHA256", "")
    if urllib.parse.urlsplit(url).scheme != "https":
        raise ValueError("MINI_MODEL_URL must use HTTPS")
    if not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise ValueError("MINI_MODEL_SHA256 must be a lowercase SHA-256 digest")
    runner_temp = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    github_env = Path(os.environ["GITHUB_ENV"])
    model = None
    try:
        digest = hashlib.sha256()
        opener = urllib.request.build_opener(HTTPSOnlyRedirect())
        with tempfile.NamedTemporaryFile(prefix="mini-build-model-", suffix=".rsnn", dir=runner_temp, delete=False) as output:
            model = Path(output.name)
            with opener.open(url, timeout=60) as response:
                size = 0
                while chunk := response.read(1024 * 1024):
                    size += len(chunk)
                    if size > 64 * 1024 * 1024:
                        raise ValueError("model exceeds the C ABI 64 MiB limit")
                    output.write(chunk)
                    digest.update(chunk)
        if digest.hexdigest() != expected:
            raise ValueError("downloaded model SHA-256 mismatch")
        if any(character in str(model) for character in "\r\n"):
            raise ValueError("model path cannot contain line breaks")
        with github_env.open("a", encoding="utf-8") as output:
            output.write(f"RSSHOGI_EMBEDDED_RSNN_PATH={model}\n")
            output.write(f"RSSHOGI_EMBEDDED_RSNN_SHA256={expected}\n")
        print("Pinned build model downloaded; its path is exported only to subsequent job steps.")
    except Exception:
        if model is not None:
            model.unlink(missing_ok=True)
        # Transport exceptions may contain a signed URL. Never expose their text.
        raise RuntimeError("build-model fetch failed; check HTTPS access, secret configuration, and SHA-256 pin") from None


if __name__ == "__main__":
    fetch()
