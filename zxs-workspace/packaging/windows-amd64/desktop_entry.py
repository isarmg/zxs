"""Entry for the portable desktop bundle only; follows the project's platform gates."""
from zipworkspace.desktop import main

if __name__ == "__main__":
    raise SystemExit(main())
