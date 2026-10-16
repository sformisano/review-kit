import unittest

from docservice import DocumentService, Forbidden, User


class ArchiveTests(unittest.TestCase):
    def setUp(self):
        self.service = DocumentService()
        self.document = self.service.create("north", "draft")

    def test_admin_archives_document(self):
        self.service.archive(User("ana", "north", "admin"), self.document.id)
        self.assertTrue(self.document.archived)
        self.assertEqual(self.service.read(User("vic", "north", "viewer"), self.document.id), "draft")

    def test_editor_cannot_archive(self):
        with self.assertRaises(Forbidden):
            self.service.archive(User("eli", "north", "editor"), self.document.id)
        self.assertFalse(self.document.archived)


class ListingTests(unittest.TestCase):
    def setUp(self):
        self.service = DocumentService()
        self.first = self.service.create("north", "a")
        self.second = self.service.create("north", "b")
        self.service.create("south", "c")
        self.archived = self.service.create("north", "d")
        self.service.archive(User("ana", "north", "admin"), self.archived.id)
        self.third = self.service.create("north", "e")

    def test_listing_contains_active_documents_of_the_organization(self):
        viewer = User("vic", "north", "viewer")
        listed = self.service.list_documents(viewer, 0, 10)
        self.assertEqual([document.id for document in listed], [self.first.id, self.second.id, self.third.id])

    def test_listing_splits_documents_into_pages(self):
        viewer = User("vic", "north", "viewer")
        self.assertEqual(len(self.service.list_documents(viewer, 0, 2)), 2)
        self.assertEqual([document.id for document in self.service.list_documents(viewer, 1, 2)], [self.third.id])

    def test_unknown_role_cannot_list(self):
        with self.assertRaises(Forbidden):
            self.service.list_documents(User("guest", "north", "guest"), 0, 10)


if __name__ == "__main__":
    unittest.main()
