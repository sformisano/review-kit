from .models import Document, Forbidden, NotFound
from .permissions import ROLE_ACTIONS, require


class DocumentService:
    def __init__(self):
        self._documents = {}
        self._next_id = 1

    def create(self, organization, text):
        document = Document(self._next_id, organization, text)
        self._documents[document.id] = document
        self._next_id += 1
        return document

    def _find(self, document_id):
        try:
            return self._documents[document_id]
        except KeyError:
            raise NotFound(document_id) from None

    def read(self, user, document_id):
        document = self._find(document_id)
        require(user, "read", document)
        return document.text

    def update_text(self, user, document_id, text):
        document = self._find(document_id)
        require(user, "update", document)
        document.text = text

    def archive(self, user, document_id):
        document = self._find(document_id)
        require(user, "archive", document)
        document.archived = True

    def list_documents(self, user, page, page_size):
        """Return one page of the user's organization's active documents."""
        if "read" not in ROLE_ACTIONS.get(user.role, frozenset()):
            raise Forbidden("read")
        visible = sorted(
            (
                document
                for document in self._documents.values()
                if document.organization == user.organization and not document.archived
            ),
            key=lambda document: document.id,
        )
        start = page * page_size
        return visible[start : start + page_size]
