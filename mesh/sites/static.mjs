// A plain static server for a small site moved to dorkyrobot1 (hibla, una): what
// `python3 -m http.server` did on the mini, on loopback only, with Range for the
// videos and no directory listings. No headers of its own beyond type, length and
// a short cache, so a page behaves as it did.
//   <program> static.mjs <port> <dir>
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
const port = Number(process.argv[2]);
const root = path.resolve(process.argv[3]);
if (!port || !fs.statSync(root).isDirectory()) { console.error('usage: static.mjs <port> <dir>'); process.exit(2); }
const types = { '.html': 'text/html; charset=utf-8', '.htm': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8', '.json': 'application/json',
  '.svg': 'image/svg+xml', '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.gif': 'image/gif',
  '.webp': 'image/webp', '.ico': 'image/x-icon', '.txt': 'text/plain; charset=utf-8', '.md': 'text/markdown; charset=utf-8',
  '.mp4': 'video/mp4', '.webm': 'video/webm', '.mov': 'video/quicktime', '.mp3': 'audio/mpeg', '.wav': 'audio/wav',
  '.m4a': 'audio/mp4', '.ogg': 'audio/ogg', '.ttf': 'font/ttf', '.otf': 'font/otf', '.woff': 'font/woff', '.woff2': 'font/woff2',
  '.wasm': 'application/wasm', '.zip': 'application/zip', '.pdf': 'application/pdf', '.xml': 'application/xml' };
http.createServer((req, res) => {
  if (req.method !== 'GET' && req.method !== 'HEAD') { res.writeHead(405); return res.end(); }
  let url;
  try { url = decodeURIComponent((req.url || '/').split('?')[0].split('#')[0]); } catch { res.writeHead(400); return res.end(); }
  let file = path.normalize(path.join(root, url));
  if (file !== root && !file.startsWith(root + path.sep)) { res.writeHead(404); return res.end(); }
  // nothing whose name starts with a dot, at any depth
  if (path.relative(root, file).split(path.sep).some(p => p.startsWith('.') && p !== '.')) { res.writeHead(404); return res.end(); }
  fs.stat(file, (err, st) => {
    if (!err && st.isDirectory()) {
      if (!url.endsWith('/')) { res.writeHead(301, { Location: url + '/' }); return res.end(); }
      file = path.join(file, 'index.html');
      try { st = fs.statSync(file); } catch { err = true; }
    }
    if (err || !st.isFile()) { res.writeHead(404, { 'Content-Type': 'text/plain' }); return res.end('not found'); }
    const head = { 'Content-Type': types[path.extname(file).toLowerCase()] || 'application/octet-stream',
      'Last-Modified': st.mtime.toUTCString(), 'Accept-Ranges': 'bytes', 'Cache-Control': 'no-cache' };
    const m = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || '');
    if (m && (m[1] || m[2])) {
      let start = m[1] ? Number(m[1]) : Math.max(0, st.size - Number(m[2]));
      let end = m[1] && m[2] ? Math.min(Number(m[2]), st.size - 1) : st.size - 1;
      if (start > end || start >= st.size) { res.writeHead(416, { 'Content-Range': `bytes */${st.size}` }); return res.end(); }
      res.writeHead(206, { ...head, 'Content-Range': `bytes ${start}-${end}/${st.size}`, 'Content-Length': end - start + 1 });
      if (req.method === 'HEAD') return res.end();
      return fs.createReadStream(file, { start, end }).pipe(res);
    }
    res.writeHead(200, { ...head, 'Content-Length': st.size });
    if (req.method === 'HEAD') return res.end();
    fs.createReadStream(file).pipe(res);
  });
}).listen(port, '127.0.0.1', () => console.log(`static on http://127.0.0.1:${port} from ${root}`));
