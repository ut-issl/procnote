// Where a device token may be handed to: a desktop app listening on this
// machine (RFC 8252 loopback redirect), never another site.

const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost", "[::1]"]);

/** Whether `callback` is an http URL on this machine's loopback interface. */
export function isLoopbackCallback(callback: string): boolean {
  let url: URL;
  try {
    url = new URL(callback);
  } catch {
    return false;
  }
  return url.protocol === "http:" && LOOPBACK_HOSTS.has(url.hostname) && !url.username;
}
