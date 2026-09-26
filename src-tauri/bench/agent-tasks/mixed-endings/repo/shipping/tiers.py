"""Shipping tiers and what a parcel costs. TIERS comes from the ops spreadsheet."""

from decimal import Decimal

TIERS = {
    "standard": {
        "max_kg": 5,
        "price": Decimal("4.90"),
    },
    "express": {
        "max_kg": 5,
        "price": Decimal("4.90"),
    },
    "freight": {
        "max_kg": 500,
        "price": Decimal("89.00"),
    },
}


class TooHeavy(ValueError):
    pass


def price_for(tier: str, weight_kg: float) -> Decimal:
    """The price of one parcel, or TooHeavy when the tier does not take it."""
    row = TIERS[tier]
    if weight_kg > row["max_kg"]:
        raise TooHeavy(f"{tier} takes up to {row['max_kg']} kg")
    return row["price"]
