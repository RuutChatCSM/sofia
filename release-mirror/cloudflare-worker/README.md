# Sofia release mirror Worker

Serves `https://release.ruut.chat/sofia/...` from the Contabo object store
(`https://eu2.contabostorage.com/<canonical-id>:ruutchat/sofia/...`), so releases
and the installer can be shared under our own domain.

## Deploy

```sh
cd release-mirror/cloudflare-worker
npm install -g wrangler   # if not installed
wrangler login
wrangler deploy
```

The `routes` entry in `wrangler.toml` binds `release.ruut.chat/*` in the
`ruut.chat` zone and Cloudflare provisions DNS + TLS automatically.

## Verify

```sh
curl -s -o /dev/null -w '%{http_code}\n' https://release.ruut.chat/sofia/channels/latest
curl -fsSL https://release.ruut.chat/sofia/install.sh | head
```

Versioned assets under `sofia/releases/<version>/` are cached at the edge;
`channels/` and installer paths always hit the origin. Range requests bypass
the cache so downloads stay resumable.
