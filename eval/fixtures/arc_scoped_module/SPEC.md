# module1 naming spec

`module1/naming.py` needs a `slugify(name)` helper:

- lowercase the input;
- collapse runs of whitespace and punctuation into single hyphens;
- strip leading/trailing hyphens;
- normalize unicode by transliterating accented Latin characters to their
  plain ASCII equivalent (e.g. "Café" -> "cafe") before slugifying.

`module2/` is legacy and out of scope for this work.
