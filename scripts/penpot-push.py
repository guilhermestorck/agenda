#!/usr/bin/env python3
"""Build design/screens.json into the connected Penpot file as native shapes.

Boards, rectangles, ellipses and text — selectable and editable by hand, which an imported
flat SVG is not. Re-runnable: boards are replaced by name rather than piling up.
"""
import json, pathlib, sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
SCRATCH = pathlib.Path("/tmp/claude-1000/-home-gui-workspace-personal-agenda/"
                       "87bdbd77-0fc5-41b9-ba62-974f0750fd44/scratchpad")
sys.path.insert(0, str(SCRATCH))
import mcp  # noqa: E402

SCREENS = json.loads((pathlib.Path(__file__).resolve().parent.parent
                      / "design" / "screens.json").read_text())

JS = """
const screen = %s;
const gap = 80;
let originX = %d;

// Replace rather than accumulate, so re-running does not litter the page with duplicates.
for (const shape of penpot.currentPage.findShapes()) {
  if (shape.name === screen.name) { shape.remove(); }
}

const board = penpot.createBoard();
board.name = screen.name;
board.x = originX; board.y = 0;
board.resize(screen.width, screen.height);
board.fills = [{ fillColor: screen.items.length ? "#1d1d20" : "#ffffff" }];

for (const item of screen.items) {
  let shape;
  if (item.kind === "text") {
    shape = penpot.createText(String(item.body));
    if (!shape) continue;
    shape.growType = "auto-width";
    shape.fontSize = String(item.size);
    // The default font carries one weight; asking for another is rejected rather than
    // approximated, so emphasis is left to the designer.
    // SVG anchors text on its baseline; Penpot positions the box, so lift it.
    shape.x = originX + item.x - (item.anchor === "middle" ? 24 : item.anchor === "end" ? 40 : 0);
    shape.y = 0 + item.y - item.size;
  } else if (item.kind === "ellipse") {
    shape = penpot.createEllipse();
    shape.x = originX + item.x; shape.y = item.y;
    shape.resize(Math.max(item.w, 1), Math.max(item.h, 1));
  } else {
    shape = penpot.createRectangle();
    shape.x = originX + item.x; shape.y = item.y;
    shape.resize(Math.max(item.w, 1), Math.max(item.h, 1));
    if (item.rx) shape.borderRadius = item.rx;
  }
  // Penpot wants opacity separately; an #rrggbbaa fill is rejected outright.
  const hex = String(item.fill);
  const solid = hex.length === 9 ? hex.slice(0, 7) : hex;
  const alphaFromHex = hex.length === 9 ? parseInt(hex.slice(7), 16) / 255 : 1;
  const opacity = (item.opacity === undefined ? 1 : item.opacity) * alphaFromHex;
  shape.fills = [{ fillColor: solid, fillOpacity: opacity }];
  board.appendChild(shape);
}
return JSON.stringify({ board: board.name, shapes: board.children.length });
"""

origin = 0
for name in sorted(SCREENS):
    screen = dict(SCREENS[name], name=name)
    bad, out = mcp.run(JS % (json.dumps(screen), origin))
    # The tool reports success while its own text says otherwise, so read the text.
    bad = bad or "failed" in out.lower() or "not valid" in out.lower()
    print(("FAILED " if bad else "built  ") + name + ": " + out.strip()[:200])
    if bad:
        break
    origin += screen["width"] + 80
