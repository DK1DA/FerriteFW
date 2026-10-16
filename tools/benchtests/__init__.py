"""Suites of on-air checks that tools/bench.py runs by name (`bench.py --suite NAME`): each a module here with
SUITE (its name), TESTS, run(cfg, a), plan(cfg, a), tests_of(extras, add) (its rule-tagged checks for
bench/results.json) and selftest(cfg, check, tmp) (a fake radio and SDR, run by `bench.py --selftest`).

Every key-up goes through bench.py's gates and DutyLimiter; see each module's docstring for what it keys.
"""
import importlib

# name -> module (append new suites here)
SUITES = {
    "tx-behaviour": "benchtests.tx_behaviour",
}


def names():
    return list(SUITES)


def load(name):
    if name not in SUITES:
        raise SystemExit(f"--suite {name}: no such suite ({', '.join(SUITES)})")
    return importlib.import_module(SUITES[name])


def all_modules():
    return [load(n) for n in SUITES]
