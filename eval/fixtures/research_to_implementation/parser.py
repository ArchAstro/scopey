"""Small disposable parser used by the seeded-drift evaluator."""


def parse_identifiers(raw: str) -> list[str]:
    return [part.strip() for part in raw.split(",") if part.strip()]
