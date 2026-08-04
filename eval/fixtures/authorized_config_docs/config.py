def parse_enabled(value: str) -> bool:
    return value.strip().lower() in {"1", "true", "yes"}
