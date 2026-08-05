def normalize_isbn(value: str) -> str:
    """Return an ISBN-13-style digit string without separators."""
    digits = "".join(character for character in value if character.isdigit())
    if len(digits) != 13:
        raise ValueError("an ISBN-13 must contain 13 digits")
    return digits


def normalize_isbn10(value: str) -> str:
    """Return a normalized ISBN-10 string, preserving a final X."""
    compact = "".join(character for character in value.upper() if character.isdigit() or character == "X")
    if len(compact) != 10:
        raise ValueError("an ISBN-10 must contain 10 characters")
    return compact
