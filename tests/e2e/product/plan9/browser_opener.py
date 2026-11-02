"""Route the isolated native client's browser opening to its Playwright operator."""

import os
import sys
from pathlib import Path
from urllib.parse import urlsplit


def main():
    target = next((value for value in sys.argv[1:] if value.startswith(("http://", "https://"))), None)
    assert target and urlsplit(target).path == "/oauth2/auth"
    path = Path(os.environ["PLAN9_NATIVE_AUTH_FILE"])
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(target)


if __name__ == "__main__":
    main()
