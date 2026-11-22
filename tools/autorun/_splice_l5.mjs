import fs from "node:fs";
const f = "apps/prismview-studio/src/components/StudioInspector.tsx";
const lines = fs.readFileSync(f, "utf8").split("\n");
// 1-based 1565..1663 inclusive
if (!lines[1564].includes("<InspectorSlider") || !lines[1662].includes("/>") || !lines[1663].includes("<InspectorSlider")) {
  throw new Error("anchor mismatch");
}
const repl = `                      <Vec3Field
                        label="Posição"
                        value={activeObj.position}
                        step={0.05}
                        onCommit={(v) => patchActiveObj((o) => ({ ...o, position: v }))}
                        onRecordStep={onRecordStep}
                        onFlushSave={onFlushSave}
                      />
                      <Vec3Field
                        label="Rotação (°)"
                        value={activeObj.rotation}
                        step={1}
                        onCommit={(v) => patchActiveObj((o) => ({ ...o, rotation: v }))}
                        onRecordStep={onRecordStep}
                        onFlushSave={onFlushSave}
                      />
                      <Vec3Field
                        label="Escala"
                        value={activeObj.scale}
                        step={0.05}
                        min={0.01}
                        onCommit={(v) => patchActiveObj((o) => ({ ...o, scale: v }))}
                        onRecordStep={onRecordStep}
                        onFlushSave={onFlushSave}
                      />`.split("\n");
lines.splice(1564, 1663 - 1564, ...repl);
fs.writeFileSync(f, lines.join("\n"));
console.log("ok");
