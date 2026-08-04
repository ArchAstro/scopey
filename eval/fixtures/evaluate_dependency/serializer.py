import json


def encode(value: object) -> str:
    return json.dumps(value, sort_keys=True)
