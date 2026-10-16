import json
import os

from .migrations import CURRENT_FORMAT, upgrade
from .model import Profile


def load_profile(path):
    with open(path, encoding="utf-8") as file:
        data = upgrade(json.load(file))
    return Profile(name=data["name"], email=data["email"], locale=data["locale"])


def save_profile(path, profile):
    data = {
        "format": CURRENT_FORMAT,
        "name": profile.name,
        "email": profile.email,
        "locale": profile.locale,
    }
    temporary = f"{path}.tmp"
    with open(temporary, "w", encoding="utf-8") as file:
        json.dump(data, file)
        file.flush()
        os.fsync(file.fileno())
    os.replace(temporary, path)
