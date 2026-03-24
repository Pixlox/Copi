export interface Transform {
  id: string;
  label: string;
  fn: (text: string) => string;
}

export const transforms: Transform[] = [
  {
    id: "strip-formatting",
    label: "Strip Formatting",
    fn: (text) => text.replace(/\s+/g, " ").trim(),
  },
  {
    id: "extract-urls",
    label: "Extract URLs",
    fn: (text) => {
      const matches = text.match(/https?:\/\/[^\s<>"{}|\\^`\[\]]+/g);
      return matches ? matches.join("\n") : "";
    },
  },
  {
    id: "extract-emails",
    label: "Extract Emails",
    fn: (text) => {
      const matches = text.match(/[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}/g);
      return matches ? matches.join("\n") : "";
    },
  },
  {
    id: "uppercase",
    label: "UPPERCASE",
    fn: (text) => text.toUpperCase(),
  },
  {
    id: "lowercase",
    label: "lowercase",
    fn: (text) => text.toLowerCase(),
  },
  {
    id: "title-case",
    label: "Title Case",
    fn: (text) =>
      text.replace(
        /\w\S*/g,
        (txt) => txt.charAt(0).toUpperCase() + txt.slice(1).toLowerCase()
      ),
  },
  {
    id: "trim",
    label: "Trim Whitespace",
    fn: (text) => text.trim(),
  },
  {
    id: "sort-lines",
    label: "Sort Lines",
    fn: (text) =>
      text
        .split("\n")
        .sort((a, b) => a.localeCompare(b))
        .join("\n"),
  },
  {
    id: "dedupe-lines",
    label: "Remove Duplicates",
    fn: (text) => {
      const seen = new Set<string>();
      return text
        .split("\n")
        .filter((line) => {
          if (seen.has(line)) return false;
          seen.add(line);
          return true;
        })
        .join("\n");
    },
  },
  {
    id: "json-pretty",
    label: "JSON Pretty Print",
    fn: (text) => {
      try {
        return JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        return text;
      }
    },
  },
];

export function getTransformedPreview(text: string, transformId: string): string {
  const transform = transforms.find((t) => t.id === transformId);
  if (!transform) return text;
  return transform.fn(text);
}
