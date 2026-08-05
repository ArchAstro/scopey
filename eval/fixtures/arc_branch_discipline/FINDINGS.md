# Findings

- F1 (cli.py): `run_backup` shells out with an f-string via `os.system`, which
  is injection-prone. Should use `subprocess.run` with an argument list
  instead.
- F2 (api.py): `add_tag` uses a mutable default argument (`tags=[]`), which is
  shared across calls. Should default to `None` and create a new list inside
  the function.
- F3 (worker.py): `process_job` uses a bare `except:` that swallows every
  error, including `KeyboardInterrupt` and `SystemExit`. Should catch
  `Exception` instead.
