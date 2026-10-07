import json
from collections import Counter
import re
import subprocess
from pathlib import Path


def run(tool, *arguments):
    result = subprocess.run([tool, *arguments], capture_output=True, text=True, timeout=20)
    if result.returncode != 0:
        raise RuntimeError(f"{tool}: {result.stderr}")
    return result.stdout


report = {"sources": {}}
for tool in ("kjv", "grb", "vul"):
    lines = run(tool, "-l").splitlines()
    start = next(i for i, line in enumerate(lines) if line == "Matthew (Mat)")
    books = []
    for entry in lines[start:]:
        name, abbreviation = re.fullmatch(r"(.+) \(([^()]+)\)", entry).groups()
        verses = []
        # Phi is a prefix shared by Philippians and Philemon in these tools.
        query = name if abbreviation == "Phi" else abbreviation
        for line in run(tool, "-W", query).splitlines():
            match = re.fullmatch(r"(\d+):(\d+)\t(.*)", line)
            if match:
                verses.append((int(match[1]), int(match[2]), match[3]))
        if not verses:
            raise RuntimeError(f"No verses: {tool} {abbreviation}")
        chapter_ids = sorted({chapter for chapter, verse, text in verses})
        assert chapter_ids == list(range(1, max(chapter_ids) + 1))
        keys = [(chapter, verse) for chapter, verse, text in verses]
        duplicate_keys = [key for key, count in Counter(keys).items() if count > 1]
        books.append({"name": name, "id": abbreviation, "query": query, "chapters": max(chapter_ids),
                      "verse_count": len(verses), "verse_keys": keys,
                      "duplicate_keys": duplicate_keys,
                      "duplicate_rows": [(chapter, verse, text) for chapter, verse, text in verses
                                         if (chapter, verse) in duplicate_keys]})
    report["sources"][tool] = books

baseline = report["sources"]["kjv"]
for tool, books in report["sources"].items():
    assert [(b["id"], b["chapters"]) for b in books] == [(b["id"], b["chapters"]) for b in baseline]

streams = [[(b["id"], chapter) for b in section for chapter in range(1, b["chapters"] + 1)]
           for section in (baseline[:4], baseline[4:])]
gospels, second = streams
final_days = 7
normal_days = len(gospels) - final_days
assert len(second) - final_days == normal_days * 2
schedule = []
for day in range(normal_days):
    schedule.append({"day": day + 1, "gospel": gospels[day],
                     "second_stream": second[day * 2:day * 2 + 2]})
for offset in range(final_days):
    schedule.append({"day": normal_days + offset + 1, "gospel": gospels[normal_days + offset],
                     "second_stream": [second[normal_days * 2 + offset]]})
assert [tuple(day["gospel"]) for day in schedule] == gospels
assert [entry for day in schedule for entry in day["second_stream"]] == second
report["schedule"] = {"gospel_chapters": len(gospels), "second_stream_chapters": len(second),
                      "normal_days": normal_days, "final_days": final_days,
                      "cycle_days": len(schedule), "assignments": schedule}
report["verse_key_differences"] = []
for index, baseline_book in enumerate(baseline):
    sets = {tool: {tuple(key) for key in books[index]["verse_keys"]}
            for tool, books in report["sources"].items()}
    union = set.union(*sets.values())
    missing = {tool: sorted(union - keys) for tool, keys in sets.items() if union - keys}
    if missing:
        report["verse_key_differences"].append({"book": baseline_book["id"], "missing_labels": missing})

output = Path(__file__).resolve().parent / "Kata-source-inspection.json"
output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print("Sources:", {tool: len(books) for tool, books in report["sources"].items()})
print("Chapter counts:", [(b["id"], b["chapters"]) for b in baseline])
print("Schedule:", {key: value for key, value in report["schedule"].items() if key != "assignments"})
print("Boundary days:", schedule[0], schedule[normal_days - 1], schedule[normal_days], schedule[-1])
print("Verse label differences:", report["verse_key_differences"])
print("Duplicate verse labels:", {tool: [(b["id"], b["duplicate_keys"]) for b in books if b["duplicate_keys"]]
                                 for tool, books in report["sources"].items()})
print("Evidence:", output)
