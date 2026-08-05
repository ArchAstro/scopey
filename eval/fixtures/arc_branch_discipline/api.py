"""Tiny in-memory API helper with a classic mutable-default bug."""


def add_tag(name, tags=[]):
    """Append name to tags and return the list."""
    tags.append(name)
    return tags
