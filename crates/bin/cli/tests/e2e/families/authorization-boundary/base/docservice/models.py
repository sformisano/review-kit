from dataclasses import dataclass


class Forbidden(Exception):
    """The user may not perform the requested action."""


class NotFound(Exception):
    """No document has the requested identifier."""


@dataclass(frozen=True)
class User:
    name: str
    organization: str
    role: str


@dataclass
class Document:
    id: int
    organization: str
    text: str
    archived: bool = False
