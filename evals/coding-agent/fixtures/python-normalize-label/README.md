# Python normalize-label repair fixture

`normalizer.py` deliberately preserves letter casing while the test requires a
normalized lowercase label. The task is to diagnose the failing test, repair the
implementation without weakening the test, and verify it with
`python3 -m unittest -v`.
