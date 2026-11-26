const test = require("node:test");
const assert = require("node:assert/strict");
const { safeTriangle } = require("../renderer/menu.cjs");

test("keeps a diagonal submenu movement inside and a vertical diversion outside", () => {
  const anchor = { x: 180, y: 140 };
  const upper = { x: 260, y: 145 };
  const lower = { x: 260, y: 215 };
  assert(safeTriangle(anchor, upper, lower, { x: 220, y: 158 }));
  assert(!safeTriangle(anchor, upper, lower, { x: 201, y: 190 }));
  assert(!safeTriangle(anchor, upper, lower, { x: 175, y: 145 }));
});
