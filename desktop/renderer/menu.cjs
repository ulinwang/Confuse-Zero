function safeTriangle(anchor, upper, lower, point) {
  if (!anchor || anchor.x >= upper.x || lower.y <= upper.y) return false;
  const dx = upper.x - anchor.x;
  const part = (point.x - anchor.x) / dx;
  if (part < 0 || part > 1) return false;
  return point.y >= anchor.y + part * (upper.y - anchor.y)
    && point.y <= anchor.y + part * (lower.y - anchor.y);
}
module.exports = { safeTriangle };
