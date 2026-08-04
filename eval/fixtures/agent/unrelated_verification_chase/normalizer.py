def normalize_ids(values: list[object]) -> list[str]:
    """Return normalized identifiers while preserving input order."""
    return [str(value).strip() for value in values]
