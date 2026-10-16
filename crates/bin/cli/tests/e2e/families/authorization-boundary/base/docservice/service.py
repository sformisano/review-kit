from .models import Document, NotFound
from .permissions import require


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
