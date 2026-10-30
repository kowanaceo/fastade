import assert from 'node:assert/strict';
import test from 'node:test';
import { serverQrPayload } from './server-qr.ts';

const server = { name: 'dev & test', host: 'example.com', port: 2222, username: 'ubuntu' };

test('server QR links use host/user and omit credentials by default', () => {
  const link = serverQrPayload(server);
  const url = new URL(link);
  assert.equal(url.protocol, 'aissh:');
  assert.deepEqual(Object.fromEntries(url.searchParams), {
    host: 'example.com', user: 'ubuntu', port: '2222', name: 'dev & test',
  });
  assert.doesNotMatch(link, /pass|key|username|hostname/i);
});

test('server QR links encode optional password and key', () => {
  const link = serverQrPayload(server, { password: 'p&ss w=rd/+', key: 'QUJD-_' });
  assert.ok(!link.includes('+'));
  const params = Object.fromEntries(new URL(link).searchParams);
  assert.equal(params.pass, 'p&ss w=rd/+');
  assert.equal(params.key, 'QUJD-_');
  assert.equal(params.name, 'dev & test');
});

test('server QR links add the jump host only when there is one', () => {
  const jump = { host: 'backend.example.com', port: 22, user: 'phone' };
  const params = Object.fromEntries(new URL(serverQrPayload({ ...server, host: 'localhost', port: 30000, jump })).searchParams);
  assert.equal(params.host, 'localhost');
  assert.equal(params.port, '30000');
  assert.equal(params.jump_host, 'backend.example.com');
  assert.equal(params.jump_port, '22');
  assert.equal(params.jump_user, 'phone');
  assert.doesNotMatch(serverQrPayload({ ...server, jump: null }), /jump/);
});
