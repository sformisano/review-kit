from dataclasses import dataclass


class UnsupportedFormat(Exception):
    """The stored profile uses a format this release cannot read."""


@dataclass(frozen=True)
class Profile:
    name: str
    email: str
