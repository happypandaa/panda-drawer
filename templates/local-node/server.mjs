import { createServer } from 'node:http';
import { createInterface } from 'node:readline';

// stdout belongs exclusively to the platform's NDJSON control protocol.
const input = createInterface({ input: process.stdin, terminal: false });
let session;
let server;
let stopping = false;
const send = (message) => process.stdout.write(`${JSON.stringify(message)}\n`);
const reply = (response, status, body) => {
  const bytes = Buffer.from(JSON.stringify(body));
  response.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Content-Length': bytes.length, 'Cache-Control': 'no-store' });
  response.end(bytes);
};

function shutdown() {
  if (stopping) return;
  stopping = true;
  if (session) send({ protocolVersion: 1, type: 'stopping', instanceId: session.instanceId });
  input.close();
  if (server) {
    server.close(() => process.exit(0));
    server.closeAllConnections();
  } else process.exit(0);
}

input.on('line', (line) => {
  try {
    const command = JSON.parse(line);
    if (!session) {
      if (command.protocolVersion !== 1 || command.type !== 'initialize' || command.instanceId !== process.env.APP_INSTANCE_ID || typeof command.token !== 'string' || !command.token) throw new Error('invalid initialize');
      session = { instanceId: command.instanceId, token: command.token };
      server = createServer((request, response) => {
        if (request.headers.authorization !== `Bearer ${session.token}`) return reply(response, 401, { error: 'unauthorized' });
        if (request.method === 'GET' && request.url === '/_platform/health') return reply(response, 200, { protocolVersion: 1, instanceId: session.instanceId, status: 'ok' });
        if (request.method === 'GET' && request.url === '/api/hello') return reply(response, 200, { message: '你的本地应用已经连通。' });
        return reply(response, 404, { error: 'not found' });
      });
      server.on('error', () => { process.stderr.write('Local application listener failed.\n'); process.exit(1); });
      server.listen(0, '127.0.0.1', () => {
        if (!stopping) send({ protocolVersion: 1, type: 'ready', instanceId: session.instanceId, host: '127.0.0.1', port: server.address().port });
      });
      return;
    }
    if (command.protocolVersion !== 1 || command.instanceId !== session.instanceId) throw new Error('invalid command identity');
    if (command.type === 'ping' && typeof command.requestId === 'string') send({ protocolVersion: 1, type: 'pong', instanceId: session.instanceId, requestId: command.requestId });
    else if (command.type === 'shutdown') shutdown();
    else throw new Error('unsupported command');
  } catch {
    process.stderr.write('Invalid local application control message.\n');
    process.exit(1);
  }
});
input.on('close', () => { if (!stopping) shutdown(); });
process.on('SIGTERM', shutdown);
process.on('SIGINT', shutdown);
