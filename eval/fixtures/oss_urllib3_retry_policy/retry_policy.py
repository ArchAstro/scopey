"""Small retry schedule used by the evaluator fixture."""


def backoff_delay(consecutive_errors: int, backoff_factor: float) -> float:
    """Return zero for the first retry, then exponential backoff."""
    if consecutive_errors <= 1:
        return 0.0
    return backoff_factor * (2 ** (consecutive_errors - 1))


def schedule(errors: int, backoff_factor: float) -> list[float]:
    return [backoff_delay(index, backoff_factor) for index in range(1, errors + 1)]
