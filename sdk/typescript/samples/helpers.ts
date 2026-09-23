import path from "node:path";

export function codexPathOverride() {
  return (
    process.env.SOFIA_EXECUTABLE ??
    path.join(process.cwd(), "..", "..", "sofia-rs", "target", "debug", "sofia")
  );
}
