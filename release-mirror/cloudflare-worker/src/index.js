// Reverse proxy that serves the Sofia release mirror from the Contabo
// object store under https://release.ruut.chat/sofia/...
//
// The upstream public URL uses Contabo's "<canonical-id>:<bucket>" path form,
// which is not a normal bucket hostname, so a Worker rewrite is used instead
// of a plain CNAME.

const UPSTREAM =
  "https://eu2.contabostorage.com/d6ff6f46e86849a2b1c82f32a5c98ba9:ruutchat";

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    const target = new URL(UPSTREAM + url.pathname);
    target.search = url.search;

    // Versioned release assets are immutable; cache them at the edge.
    // Range requests (resumable downloads) bypass the cache so responses
    // keep their partial-content semantics.
    const immutable = /^\/sofia\/releases\/[^/]+\//.test(url.pathname);
    const hasRange = request.headers.has("range");
    if (!immutable || hasRange || request.method !== "GET") {
      return fetch(target, { method: request.method, headers: request.headers });
    }

    const cache = caches.default;
    const cacheKey = new Request(target.toString(), { method: "GET" });
    let response = await cache.match(cacheKey);
    if (!response) {
      response = await fetch(target, {
        method: "GET",
        headers: request.headers,
      });
      if (response.ok) {
        response = new Response(response.body, response);
        response.headers.set(
          "Cache-Control",
          "public, max-age=31536000, immutable",
        );
        ctx.waitUntil(cache.put(cacheKey, response.clone()));
      }
    }
    return response;
  },
};
