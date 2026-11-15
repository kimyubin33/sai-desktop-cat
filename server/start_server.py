"""Windows frozen executable entry point."""
from server.cli import main

if __name__ == "__main__":
    raise SystemExit(main())
