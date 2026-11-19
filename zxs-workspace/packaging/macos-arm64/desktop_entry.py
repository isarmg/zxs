"""Desktop-only application entry, not a shell, installer or volume helper."""
from zipworkspace.desktop import main

if __name__ == "__main__":
    raise SystemExit(main())
