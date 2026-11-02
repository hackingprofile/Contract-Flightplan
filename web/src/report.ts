export type Lifecycle =
  | "proposal"
  | "reviewed"
  | "approved"
  | "stale"
  | "rejected"
  | "failed"
  | "superseded";

export interface ReportDocument {
  schema_version: string;
  kind: "report";
  approval_state: string;
  diff: { kind: string; source: { path: string; digest: string } };
}

export function assertReport(value: unknown): ReportDocument {
  if (typeof value !== "object" || value === null) {
    throw new Error("report is not an object");
  }
  const rec = value as Record<string, unknown>;
  if (rec.kind !== "report" || rec.schema_version !== "1") {
    throw new Error("not a v1 report");
  }
  return value as ReportDocument;
}

export const NAV = [
  "verdict",
  "summary",
  "timeline",
  "delta",
  "examples",
  "rules",
  "uncertainty",
  "approval",
  "fixtures",
] as const;
