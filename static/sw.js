// procnote's service worker: keeps the app itself available offline, so a
// page can be reloaded (or opened) without the network. Data never goes
// through here: /api, /collab, and phone upload links always use the network,
// and recordings are kept by the app in IndexedDB (src/lib/offline/).
//
// - Built assets (/assets/*) have content hashes in their names, so a cached
//   copy is always right: cache first.
// - Pages (navigations) come from the network when it answers, falling back
//   to the last index.html, since the app routes in the browser.

const CACHE = "procnote-app-v1";
const INDEX = "/index.html";

function isAsset(url) {
  return url.pathname.startsWith("/assets/");
}

function isData(url) {
  return (
    url.pathname.startsWith("/api/") ||
    url.pathname.startsWith("/collab") ||
    url.pathname.startsWith("/upload/")
  );
}

/** Keep a copy of a normal same-origin response (not a login redirect or error). */
async function keep(request, response) {
  if (response.ok && response.type === "basic") {
    const cache = await caches.open(CACHE);
    await cache.put(request, response.clone());
  }
  return response;
}

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.add(INDEX))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(names.filter((name) => name !== CACHE).map((name) => caches.delete(name))),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const { request } = event;
  const url = new URL(request.url);
  if (request.method !== "GET" || url.origin !== self.location.origin || isData(url)) return;

  if (request.mode === "navigate") {
    event.respondWith(
      fetch(request)
        .then((response) => keep(new Request(INDEX), response))
        .catch(async () => (await caches.match(INDEX)) ?? Response.error()),
    );
    return;
  }
  if (isAsset(url)) {
    event.respondWith(
      caches
        .match(request)
        .then((cached) => cached ?? fetch(request).then((r) => keep(request, r))),
    );
  }
});
