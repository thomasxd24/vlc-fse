'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { parseStatus, AUTH_URL } = require('../src/tailscale');

// Trimmed from real `tailscale status --json` output.
const running = {
  BackendState: 'Running',
  AuthURL: '',
  Self: { HostName: 'LEGION-GO', DNSName: 'legion-go.tail1234.ts.net.', TailscaleIPs: ['100.101.102.103', 'fd7a:115c:a1e0::1'], UserID: 42, Online: true },
  User: { 42: { LoginName: 'thomas@example.com' } },
  CurrentTailnet: { Name: 'thomas@example.com' },
  Peer: {
    'nodekey:a': { HostName: 'nas', DNSName: 'nas.tail1234.ts.net.', TailscaleIPs: ['100.64.0.2'], Online: true, ExitNodeOption: true, ExitNode: true },
    'nodekey:b': { HostName: 'Pixel 8', DNSName: 'pixel-8.tail1234.ts.net.', TailscaleIPs: ['100.64.0.3'], Online: false },
    'nodekey:c': { HostName: 'vps', DNSName: 'vps.tail1234.ts.net.', TailscaleIPs: ['100.64.0.4'], Online: false, ExitNodeOption: true }
  }
};

test('tailscale status: connected, with an exit node in use', () => {
  const s = parseStatus(JSON.stringify(running));
  assert.equal(s.state, 'connected');
  assert.equal(s.hostName, 'legion-go');
  assert.equal(s.ip, '100.101.102.103');
  assert.equal(s.user, 'thomas@example.com');
  assert.equal(s.peersOnline, 1);
  assert.equal(s.peersTotal, 3);
  assert.deepEqual(s.exitNode, { name: 'nas', ip: '100.64.0.2' });
  assert.deepEqual(s.exitNodes.map((n) => [n.name, n.online, n.active]), [['nas', true, true], ['vps', false, false]]);
});

test('tailscale status: stopped and signed out', () => {
  assert.equal(parseStatus({ BackendState: 'Stopped', Self: {} }).state, 'stopped');
  const out = parseStatus({ BackendState: 'NeedsLogin', AuthURL: 'https://login.tailscale.com/a/abc123', Self: {} });
  assert.equal(out.state, 'needsLogin');
  assert.equal(out.authUrl, 'https://login.tailscale.com/a/abc123');
  assert.equal(out.exitNode, null);
});

test('finds the sign-in URL in `tailscale up` output', () => {
  const text = '\nTo authenticate, visit:\n\n\thttps://login.tailscale.com/a/1a2b3c4d5e6f\n\n';
  assert.equal(AUTH_URL.exec(text)[0], 'https://login.tailscale.com/a/1a2b3c4d5e6f');
});
