/**
 * Format an ISO 8601 timestamp string for display.
 * Shows date and time in the user's local timezone.
 */
export function formatTimestamp(iso: string): string {
  const date = new Date(iso);
  return date.toLocaleString([], {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
