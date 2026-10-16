import json
import os
import tempfile
import unittest

from profilestore import Profile, UnsupportedFormat, load_profile, save_profile


class StorageTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.directory.name, "profile.json")

    def tearDown(self):
        self.directory.cleanup()

    def write(self, data):
        with open(self.path, "w", encoding="utf-8") as file:
            json.dump(data, file)

    def test_saved_profile_loads_unchanged(self):
        profile = Profile(name="Ada Lovelace", email="ada@example.org", locale="en-GB")
        save_profile(self.path, profile)
        self.assertEqual(load_profile(self.path), profile)

    def test_saved_file_uses_format_2(self):
        save_profile(self.path, Profile(name="Ada", email="ada@example.org"))
        with open(self.path, encoding="utf-8") as file:
            self.assertEqual(
                json.load(file),
                {"format": 2, "name": "Ada", "email": "ada@example.org", "locale": "en"},
            )

    def test_format_1_profile_loads_with_default_locale(self):
        self.write({"format": 1, "name": "Ada Lovelace", "email": "ada@example.org"})
        self.assertEqual(
            load_profile(self.path),
            Profile(name="Ada Lovelace", email="ada@example.org", locale="en"),
        )

    def test_loading_leaves_the_file_unchanged(self):
        self.write({"format": 1, "name": "Ada Lovelace", "email": "ada@example.org"})
        with open(self.path, "rb") as file:
            before = file.read()
        load_profile(self.path)
        with open(self.path, "rb") as file:
            self.assertEqual(file.read(), before)

    def test_unknown_formats_are_rejected(self):
        for version in [None, "1", True, 0, 3]:
            self.write({"format": version, "name": "Ada", "email": "ada@example.org"})
            with self.assertRaises(UnsupportedFormat, msg=repr(version)):
                load_profile(self.path)


if __name__ == "__main__":
    unittest.main()
