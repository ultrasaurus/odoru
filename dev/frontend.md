# Frontend

Frontend files are in `app/frontend/src/` (`*.ts` and `*.css`) 

The logic is tightly coupled across these files:

- `app/frontend/src/main.ts` — view logic, DOM construction, player wiring
- `app/frontend/src/player.ts` — AudioContext, WebSocket, seek/highlight logic
- `app/frontend/src/markdown.ts` — markdown rendering, sentence span weaving
- `app/frontend/src/types.ts` — shared TypeScript interfaces
- `app/frontend/src/style.css` — all styles; class names are shared across the above
