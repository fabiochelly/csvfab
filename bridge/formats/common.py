"""Ce que les lecteurs (xlsx, SQLite) partagent pour écrire leurs CSV."""


def number_text(raw, comma):
    """Un nombre réécrit au plus court (1234.0 -> 1234), virgule décimale si comma
    (un CSV en « ; », la convention des tableurs français) ; raw tel quel s'il n'en est pas un."""
    try:
        x = float(raw)
    except ValueError:
        return raw
    s = str(int(x)) if x == int(x) and abs(x) < 1e15 else repr(x)
    return s.replace(".", ",") if comma else s
