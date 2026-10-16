# Profile Store

Profile Store saves user profiles as JSON files on the local disk. Saved files
outlive application releases, so the stored format is a compatibility
contract. [docs/storage-format.md](docs/storage-format.md) defines it.

## Checks

Run `python3 -m unittest discover -s tests -t .`. The package uses only the
Python standard library and its tests need no network.
