"""Small CLI helper with a shell-injection-prone system call."""

import os


def run_backup(filename):
    """Back up a file using the system `cp` command."""
    os.system(f"cp {filename} {filename}.bak")
