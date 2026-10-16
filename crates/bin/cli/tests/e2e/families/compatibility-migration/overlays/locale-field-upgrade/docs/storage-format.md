# Stored profile format

Profile files are JSON objects with an integer `format` field. This document is
the compatibility contract for every released format.

## Rules

- `load_profile` reads every released format listed below and returns the
  current `Profile`.
- `save_profile` writes only the newest format, replacing the file atomically.
- A new format adds a section below and an upgrade step in
  `profilestore/migrations.py` that converts the previous format. Released
  sections are never edited.
- Loading never modifies the file. A missing, non-integer or newer `format`
  fails with `UnsupportedFormat`.

## Format 1

    {"format": 1, "name": "Ada Lovelace", "email": "ada@example.org"}

- `name`: display name, a non-empty string.
- `email`: contact address, a string.

## Format 2

    {"format": 2, "name": "Ada Lovelace", "email": "ada@example.org", "locale": "en-GB"}

- `name`: display name, a non-empty string.
- `email`: contact address, a string.
- `locale`: preferred language as a BCP 47 tag, for example `en-GB`.

A format 1 file upgrades to format 2 with `locale` set to `en`.
