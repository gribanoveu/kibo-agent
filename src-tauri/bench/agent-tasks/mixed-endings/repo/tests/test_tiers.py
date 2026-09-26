import unittest
from decimal import Decimal

from shipping.tiers import TooHeavy, price_for


class PriceTest(unittest.TestCase):
    def test_standard(self):
        self.assertEqual(price_for("standard", 2), Decimal("4.90"))

    def test_express_costs_more_than_standard(self):
        self.assertEqual(price_for("express", 2), Decimal("12.50"))

    def test_freight_takes_heavy_parcels(self):
        self.assertEqual(price_for("freight", 120), Decimal("89.00"))

    def test_too_heavy(self):
        with self.assertRaises(TooHeavy):
            price_for("standard", 6)


if __name__ == "__main__":
    unittest.main()
