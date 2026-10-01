"""Polices à chasse fixe : celles qui sont installées, et l'installation d'une police du catalogue."""

from .. import fonts
from ..httpd import HttpError


def listing(req):
    return {"mono": fonts.mono_families(), "catalog": fonts.catalog()}


def install(req):
    """Le corps est ignoré : seul l'identifiant du catalogue compte (jamais une adresse)."""
    try:
        return fonts.install(req.arg("id"))
    except Exception as e:
        raise HttpError(502, str(e))
