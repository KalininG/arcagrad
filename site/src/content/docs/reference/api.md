---
title: REST API
description: Arcagrad's REST API and where to find the interactive reference.
---

Arcagrad exposes a native REST API under `/api`. The OpenAPI specification is generated
directly from the server code, so it never drifts from the implementation.

## Interactive reference

Your running instance serves the full, browsable API docs:

- **`/api/docs`** — an offline Swagger UI you can explore and try calls from.
- **`/api/openapi.json`** — the raw OpenAPI document, for code generators and tooling.

## Authentication

The web app uses a session cookie. For scripts and integrations, create a personal **API key**
(see [Administration](/administration/)) and send it as a Bearer token:

```bash
curl -H "Authorization: Bearer <your-api-key>" http://localhost:3000/api/items
```
