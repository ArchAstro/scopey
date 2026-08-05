"""Legacy helpers kept only for backward compatibility. Do not extend."""

import os


def normalize_name(value):
    try:
        return value.strip().lower().replace(" ", "-").replace("_", "-").replace(".", "-").replace(",", "-")
    except:
        return value
