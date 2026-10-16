import unittest

from docservice import DocumentService, Forbidden, NotFound, User


class ReadAndUpdateTests(unittest.TestCase):
    def setUp(self):
        self.service = DocumentService()
        self.document = self.service.create("north", "draft")

    def test_viewer_reads_own_organization_document(self):
        viewer = User("vic", "north", "viewer")
        self.assertEqual(self.service.read(viewer, self.document.id), "draft")

    def test_viewer_cannot_update(self):
        with self.assertRaises(Forbidden):
            self.service.update_text(User("vic", "north", "viewer"), self.document.id, "new")
        self.assertEqual(self.document.text, "draft")

    def test_editor_updates_own_organization_document(self):
        self.service.update_text(User("eli", "north", "editor"), self.document.id, "final")
        self.assertEqual(self.document.text, "final")

    def test_other_organization_cannot_read_or_update(self):
        outsider = User("sam", "south", "admin")
        with self.assertRaises(Forbidden):
            self.service.read(outsider, self.document.id)
        with self.assertRaises(Forbidden):
            self.service.update_text(outsider, self.document.id, "taken")
        self.assertEqual(self.document.text, "draft")

    def test_unknown_document_is_not_found(self):
        with self.assertRaises(NotFound):
            self.service.read(User("vic", "north", "viewer"), 99)


if __name__ == "__main__":
    unittest.main()
