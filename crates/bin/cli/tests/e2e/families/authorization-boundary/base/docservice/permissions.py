from .models import Forbidden

ROLE_ACTIONS = {
    "viewer": frozenset({"read"}),
    "editor": frozenset({"read", "update"}),
    "admin": frozenset({"read", "update"}),
}


def require(user, action, document):
    """Raise Forbidden unless the user may perform the action on the document."""
    if document.organization != user.organization:
        raise Forbidden(action)
    if action not in ROLE_ACTIONS.get(user.role, frozenset()):
        raise Forbidden(action)
