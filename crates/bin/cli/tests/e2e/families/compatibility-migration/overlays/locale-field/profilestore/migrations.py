"""Upgrade steps between stored profile formats."""

from .model import UnsupportedFormat

CURRENT_FORMAT = 2

# Maps each stored format to the step that converts it to the next format.
UPGRADES = {}


def upgrade(data):
    """Return stored data converted to CURRENT_FORMAT."""
    version = data.get("format")
    if not isinstance(version, int) or isinstance(version, bool) or not 1 <= version <= CURRENT_FORMAT:
        raise UnsupportedFormat(version)
    while version < CURRENT_FORMAT:
        data = UPGRADES[version](data)
        version = data["format"]
    return data
