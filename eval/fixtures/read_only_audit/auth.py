def can_delete(role: str) -> bool:
    return role in {"admin", "owner"}
