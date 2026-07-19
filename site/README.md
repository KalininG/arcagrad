# Arcagrad website

The marketing site and user documentation for [arcagrad.com](https://arcagrad.com). It is a static Astro site using Starlight for the documentation pages.

## Local development

```sh
npm ci
npm run dev
```

Astro serves the site at `http://localhost:4321`.

## Production build

```sh
npm ci
npm run verify
npm run preview
```

The deployable output is `dist/`. Astro only includes generated pages, documentation under `src/content/docs/`, and files under `public/`; files elsewhere in the repository are not copied into the site.

`npm run verify` builds the site and rejects source maps, source files, local
filesystem paths, and common credential formats in `dist/`.
You can also inspect the output directly:

```sh
find dist -type f | sort
rg -n -i '/Users/|BEGIN .*PRIVATE KEY|github_pat_|ghp_|sk-' dist
```

The second command should return no matches. Placeholder paths and tokens in user documentation should use visibly fake values such as `<your-api-key>`.

## Structure

- `src/pages/` contains the marketing and download pages.
- `src/content/docs/` contains public user documentation.
- `src/components/` contains Starlight component overrides.
- `src/styles/` contains documentation theme overrides.
- `public/` is copied directly to the build output.
- `astro.config.mjs` contains the canonical production origin.
- `LAUNCH.md` contains the Cloudflare Pages deployment checklist.

## Deployment files

`public/robots.txt` publishes the sitemap location. `public/_headers` supplies the security and immutable-asset cache headers understood by Cloudflare Pages.

Do not place environment files, credentials, source maps, or local-only exports
under `src/content/docs/` or `public/`; those directories are intentionally
public.
