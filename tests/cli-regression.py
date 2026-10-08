"""Temporary end-to-end tests for importing print reading progress."""
from pathlib import Path
import datetime
import json
import os
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
binary = Path(os.environ.get('KATA_BIN', str(root / "zig-out/bin/kata")))

def run(*args, ok=True):
    result = subprocess.run([str(binary), *map(str, args)], cwd=root, capture_output=True, text=True, timeout=30)
    assert (result.returncode == 0) == ok, (args, result.returncode, result.stdout, result.stderr)
    return result

with tempfile.TemporaryDirectory(prefix=".prototype-jump-test-", dir=root) as tmp:
    path = Path(tmp) / "state.json"
    base = ("--state", path)
    preview = run(*base, "--start-day", 88).stdout
    assert "John:20" in preview and "Revelation:21" in preview and "87" in preview
    assert not path.exists(), "preview wrote state"
    saved = run(*base, "--start-day", 88, "--confirm").stdout
    state = json.loads(path.read_text())
    assert state["next_day"] == 87 and state["completed_on"] == 0
    assert all(p == {"row": 0, "line": 0} for p in state["positions"])
    assert "Saved" in saved
    text = run(*base, "--dump").stdout
    assert "day 88/89" in text and "John 20:1" in text and "Revelation 21:1" in text
    assert "kjv:" in text and "grb:" in text and "vul:" in text
    print("PASS day-88 preview, confirmed import, and real pending John20/Revelation21 text")

    before = path.read_bytes()
    run(*base, "--start-day", 20)
    assert path.read_bytes() == before, "preview modified existing progress"
    for day in (0, 90, -1, "oops", "+88", "184467440737095516160"):
        run(*base, "--start-day", day, "--confirm", ok=False)
        assert path.read_bytes() == before
    for args in (("--start-day",), ("--confirm",), ("--start-day", 88, "--confirm", "--dump"), ("--start-day", 88, "--passage", "John:1"), ("--start-day", 88, "--complete"), ("--start-day", 88, "--check-plan")):
        run(*base, *args, ok=False)
        assert path.read_bytes() == before
    print("PASS previews, invalid input, and conflicting flags never change progress")

    run(*base, "--complete")
    completed = json.loads(path.read_text())
    assert completed["next_day"] == 88
    assert completed["completed_on"] == int(datetime.date.today().strftime("%Y%m%d"))
    run(*base, "--complete", ok=False)
    assert "day 88/89" in run(*base, "--dump").stdout
    yesterday = int((datetime.date.today() - datetime.timedelta(days=1)).strftime("%Y%m%d"))
    completed["completed_on"] = yesterday
    path.write_text(json.dumps(completed))
    text = run(*base, "--dump").stdout
    assert "day 89/89" in text and "John 21:1" in text and "Revelation 22:1" in text
    print("PASS imported selected day completes normally and exposes day89 next date")

    # Backward repositioning preserves source preferences and makes its day pending.
    completed.update(next_day=100, focus=1, linked=False, enabled=[True, True, False])
    path.write_text(json.dumps(completed))
    run(*base, "--start-day", 88, "--confirm")
    repositioned = json.loads(path.read_text())
    assert repositioned["next_day"] == 176 and repositioned["completed_on"] == 0
    assert repositioned["focus"] == 1 and repositioned["linked"] is False
    assert repositioned["enabled"] == [True, True, False]
    run(*base, "--start-day", 1, "--confirm")
    assert json.loads(path.read_text())["next_day"] == 89
    print("PASS repeat-cycle accounting, backward repositioning, and preserved pane preferences")

    custom = Path(tmp) / "custom.json"
    custom.write_text(json.dumps({"name":"Short", "repeat":False, "streams":[{"books":[{"name":"John","chapters":2}]}], "phases":[{"days":2,"rates":[1]}]}))
    other = Path(tmp) / "custom-state.json"
    run("--plan", custom, "--state", other, "--start-day", 2, "--confirm")
    assert "day 2/2" in run("--plan", custom, "--state", other, "--dump").stdout
    data = json.loads(other.read_text())
    data.update(next_day=2, completed_on=yesterday)
    other.write_text(json.dumps(data))
    run("--plan", custom, "--state", other, "--start-day", 1, "--confirm")
    assert json.loads(other.read_text())["next_day"] == 0
    print("PASS arbitrary plans and restarting a completed nonrepeating plan")
print("ALL IMPORT-PROGRESS CHECKS PASSED")


