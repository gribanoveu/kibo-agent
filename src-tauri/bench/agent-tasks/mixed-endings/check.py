"""Byte for byte: the express price changed, every other line as it was — its line ending included."""
import os
import sys

orig, work = os.environ["ORIG"], os.environ["WORKSPACE"]
name = os.path.join("shipping", "tiers.py")
want = open(os.path.join(orig, name), "rb").read()
express = want.index(b'"express"')
at = want.index(b'"price": Decimal("4.90"),\r\n', express)
want = want[:at] + b'"price": Decimal("12.50"),\r\n' + want[at + len(b'"price": Decimal("4.90"),\r\n'):]
got = open(os.path.join(work, name), "rb").read()
if got != want:
    crlf, expected = got.count(b"\r\n"), want.count(b"\r\n")
    print(f"{name} differs from the expected bytes ({crlf} CRLF line endings, {expected} expected)")
    sys.exit(1)
