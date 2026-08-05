"""Background worker loop with an overly broad error handler."""


def process_job(job, handler):
    """Run handler(job) and return its result, or None if it raised."""
    try:
        return handler(job)
    except:
        return None
