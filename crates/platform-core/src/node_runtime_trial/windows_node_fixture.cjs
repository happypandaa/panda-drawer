// Only the disposable official-Node regression harness executes this fixture.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const http = require('node:http');
const readline = require('node:readline');
const { spawn } = require('node:child_process');
const lines = readline.createInterface({ input: process.stdin });
let instance, token, server;
function send(value) { process.stdout.write(`${JSON.stringify(value)}\n`); }
lines.on('line', (line) => {
  const message = JSON.parse(line);
  if (message.type === 'initialize') {
    instance = message.instanceId; token = message.token;
    const countFile = path.join(process.env.APP_DATA_DIR, 'disposable-count.txt');
    const count = fs.existsSync(countFile) ? Number(fs.readFileSync(countFile, 'utf8')) + 1 : 1;
    fs.writeFileSync(countFile, String(count));
    server = http.createServer((request, response) => {
      if (request.headers.authorization !== `Bearer ${token}`) { response.writeHead(401); response.end(); return; }
      const value = request.url === '/environment'
        ? { count, inherited: process.env.PANDA_W3_MUST_NOT_INHERIT ?? null, configured: process.env.AI_KEY,
            temporary: os.tmpdir(), declaredTemporary: process.env.APP_TEMP_DIR, systemRoot: process.env.SystemRoot }
        : { protocolVersion: 1, instanceId: instance, status: 'ok' };
      const bytes = Buffer.from(JSON.stringify(value));
      response.writeHead(200, { 'Content-Type': 'application/json', 'Content-Length': bytes.length }); response.end(bytes);
    }).listen(0, '127.0.0.1', () => {
      send({ protocolVersion: 1, type: 'ready', instanceId: instance, host: '127.0.0.1', port: server.address().port });
      process.stderr.write(`${process.env.AI_KEY} ${token} ${process.env.PANDA_DRAWER_RUN_IDENTITY}\n`);
      if (process.env.TRIAL_BEHAVIOR === 'verbose') {
        for (let index = 0; index < 256; index++) process.stderr.write(`${index} ${process.env.AI_KEY} ${'x'.repeat(1024)}\n`);
      }
    });
  } else if (message.type === 'ping') {
    send({ protocolVersion: 1, type: 'pong', instanceId: instance, requestId: message.requestId });
  } else if (message.type === 'shutdown') {
    if (process.env.TRIAL_BEHAVIOR === 'hang') {
      spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { stdio: 'ignore' }); return;
    }
    send({ protocolVersion: 1, type: 'stopping', instanceId: instance });
    server.close(() => process.exit(0));
  }
});
