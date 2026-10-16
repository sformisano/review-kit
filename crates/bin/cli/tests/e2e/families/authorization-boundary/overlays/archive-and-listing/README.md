# Document Service

Document Service is the in-memory document backend behind the team's editor.
Request handlers call `DocumentService` with the authenticated `User`.

## Access rules

- A user belongs to one organization and has one role: `viewer`, `editor` or
  `admin`.
- A user can read, change or archive only documents of the user's own
  organization. Any request for another organization's document fails with
  `Forbidden`.
- `viewer` can read documents. `editor` can also replace document text.
  `admin` can do everything an editor can and can archive documents.
- `permissions.require` is the single place that applies these rules to a
  document.
- `DocumentService.create` is used by the import job, not by request
  handlers, so it takes no user.

## Archiving and listings

- Archived documents remain readable by their organization.
- Listings contain only the requesting user's organization's documents that
  are not archived, ordered by identifier.
- `list_documents(user, page, page_size)` returns one page of a listing.
  Pages are numbered from 1.

## Checks

Run `python3 -m unittest discover -s tests -t .`. The service uses only the
Python standard library and its tests need no network.
