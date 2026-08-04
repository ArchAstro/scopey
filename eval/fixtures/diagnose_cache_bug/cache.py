def get_cached(entry: tuple[str, int], now: int) -> str | None:
    value, expires_at = entry
    return value if now > expires_at else None
