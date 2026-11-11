#!/usr/bin/env python3
"""Build fixtures/sample.ndjson from live Wikidata.

One-off developer tool: its output is committed, and neither tests nor CI run it.

Sources:
  * fixtures/titles.txt     English Wikipedia titles, resolved via wbgetentities.
  * SHARED_NAMES below      every human whose en label or alias is exactly one of
                            these names (via the query service), so the fixture
                            holds the real confusable neighbours of mononyms.

Each entity is trimmed to the fields the builder reads, plus a few it must
ignore, and written one JSON object per line: the same shape as a line of the
Wikidata JSON dump, minus the dump's enclosing "[" / "]" and trailing commas.
"""

import json
import sys
import time
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TITLES = ROOT / "fixtures" / "titles.txt"
OUT = ROOT / "fixtures" / "sample.ndjson"

API = "https://www.wikidata.org/w/api.php"
SPARQL = "https://query.wikidata.org/sparql"
UA = "hundred-fixture-fetch/0.1 (Name100 lookup server; one-off fixture build)"

SHARED_NAMES = ["Jennifer", "Madonna", "Cher", "Beyoncé", "Selena", "Adele", "Prince"]
MAX_PER_SHARED_NAME = 12

KEEP_LANGS = ("en", "mul", "fr", "de")
KEEP_CLAIMS = (
    "P31", "P21", "P569", "P570",
    # category attributes, see crates/core/src/attribute.rs
    "P106", "P27", "P166", "P39", "P641", "P1344", "P136", "P1303", "P101", "P97",
)


def get_json(url, params):
    full = url + "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(full, headers={"User-Agent": UA, "Accept": "application/json"})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                return json.load(resp)
        except Exception as exc:  # noqa: BLE001 - retry anything transient
            if attempt == 4:
                raise
            print(f"  retry after error: {exc}", file=sys.stderr)
            time.sleep(65 if "429" in str(exc) else 2 * (attempt + 1))


def chunks(items, n):
    for i in range(0, len(items), n):
        yield items[i : i + n]


def fetch_by_titles(titles):
    entities, missing = {}, []
    for batch in chunks(titles, 50):
        data = get_json(API, {
            "action": "wbgetentities", "format": "json", "sites": "enwiki",
            "titles": "|".join(batch), "props": "labels|aliases|descriptions|claims|sitelinks",
        })
        found_titles = set()
        for key, ent in data.get("entities", {}).items():
            if "missing" in ent or not key.startswith("Q"):
                continue
            entities[key] = ent
            found_titles.add(ent.get("sitelinks", {}).get("enwiki", {}).get("title"))
        missing += [t for t in batch if t not in found_titles]
        time.sleep(0.5)
    return entities, missing


def fetch_by_ids(qids):
    entities = {}
    for batch in chunks(qids, 50):
        data = get_json(API, {
            "action": "wbgetentities", "format": "json", "ids": "|".join(batch),
            "props": "labels|aliases|descriptions|claims|sitelinks",
        })
        for key, ent in data.get("entities", {}).items():
            if "missing" not in ent:
                entities[key] = ent
        time.sleep(0.5)
    return entities


def humans_named(names):
    """For each name, the QIDs of humans whose en label or alias is exactly that
    name, most sitelinks first. One query for all names: the query service is
    heavily rate limited."""
    values = " ".join(json.dumps(n, ensure_ascii=False) + "@en" for n in names)
    query = f"""
    SELECT DISTINCT ?name ?item ?links WHERE {{
      VALUES ?name {{ {values} }}
      {{ ?item rdfs:label ?name }} UNION {{ ?item skos:altLabel ?name }}
      ?item wdt:P31 wd:Q5 ; wikibase:sitelinks ?links .
      FILTER(?links > 0)
    }} ORDER BY DESC(?links)
    """
    data = get_json(SPARQL, {"query": query, "format": "json"})
    out = {n: [] for n in names}
    for row in data["results"]["bindings"]:
        qid = row["item"]["value"].rsplit("/", 1)[1]
        bucket = out[row["name"]["value"]]
        if qid not in bucket and len(bucket) < MAX_PER_SHARED_NAME:
            bucket.append(qid)
    return out


def trim(ent):
    def langs(field):
        return {k: v for k, v in ent.get(field, {}).items() if k in KEEP_LANGS}

    claims = {}
    for prop in KEEP_CLAIMS:
        kept = []
        for st in ent.get("claims", {}).get(prop, []):
            snak = st.get("mainsnak", {})
            slim = {"snaktype": snak.get("snaktype"), "property": prop}
            if "datavalue" in snak:
                slim["datavalue"] = snak["datavalue"]
            kept.append({"mainsnak": slim, "type": "statement", "rank": st.get("rank", "normal")})
        if kept:
            claims[prop] = kept

    sitelinks = {
        site: {"site": site, "title": link["title"]}
        for site, link in sorted(ent.get("sitelinks", {}).items())
    }
    return {
        "type": ent.get("type", "item"),
        "id": ent["id"],
        "labels": langs("labels"),
        "descriptions": {k: v for k, v in ent.get("descriptions", {}).items() if k == "en"},
        "aliases": langs("aliases"),
        "claims": claims,
        "sitelinks": sitelinks,
    }


def main():
    titles = []
    for line in TITLES.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and line not in titles:
            titles.append(line)

    print(f"resolving {len(titles)} titles", file=sys.stderr)
    entities, missing = fetch_by_titles(titles)

    extra = []
    for name, qids in humans_named(SHARED_NAMES).items():
        print(f"  humans named {name!r}: {len(qids)}", file=sys.stderr)
        extra += [q for q in qids if q not in entities and q not in extra]
    entities.update(fetch_by_ids(extra))

    ordered = sorted(entities.values(), key=lambda e: int(e["id"][1:]))
    with OUT.open("w", encoding="utf-8") as fh:
        for ent in ordered:
            fh.write(json.dumps(trim(ent), ensure_ascii=False, separators=(",", ":")) + "\n")

    print(f"wrote {len(ordered)} entities to {OUT} ({OUT.stat().st_size / 1e6:.2f} MB)", file=sys.stderr)
    if missing:
        print("titles with no Wikidata item (check spelling / redirects):", file=sys.stderr)
        for title in missing:
            print(f"  {title}", file=sys.stderr)


if __name__ == "__main__":
    main()
