// Tests for the Web Bluetooth page's protocol client (tools/webble/index.html, the ferrite-core block), under node:
//
//   node tools/webble/test.mjs                              # framing (against tools/ferrite_link.py) and the fake radio
//   node tools/webble/test.mjs --sim tcp:127.0.0.1:7300     # also against the simulator, in 20-byte chunks
//
// The simulator: cargo run -p ferrite-sim -- --term-headless 600 --term-tcp 7300 (its TCP port is the cable).
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { createConnection } from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const here = dirname(fileURLToPath(import.meta.url));
const html = readFileSync(join(here, "index.html"), "utf8");
const src = html.slice(html.indexOf("/* ferrite-core:begin"), html.indexOf("/* ferrite-core:end */"));
const module = { exports: null };
new Function("module", src)(module);
const F = module.exports;
let passed = 0;
const ok = (name) => { passed++; console.log(`ok   ${name}`); };

// ---- Framing ----
assert.equal(F.crc16(Array.from(Buffer.from("123456789"))), 0x29B1, "CRC-16/CCITT-FALSE check value");
ok("crc16 check value");

for (const n of [0, 1, 5, 253, 254, 255, 300, 600]) {
  const data = Array.from({ length: n }, (_, i) => (i * 37 + 11) % 7 === 0 ? 0 : (i * 13 + 1) & 0xFF);
  const enc = F.cobsEncode(data);
  assert.ok(!enc.includes(0), "no 0x00 inside");
  assert.deepEqual(F.cobsDecode(enc), data, `round trip ${n}`);
}
const ones = Array(600).fill(1);
assert.deepEqual(F.cobsDecode(F.cobsEncode(ones)), ones);
ok("COBS round trips (runs past 254)");

// The same bytes as the reference PC side.
const py = `
import sys, struct
sys.path.insert(0, ${JSON.stringify(join(here, ".."))})
from ferrite_link import cobs_encode, crc16
for ty, seq, p in [(0x01, 1, bytes([2]) + bytes(8) + b"webble"), (0x30, 200, bytes([0x80, 2])), (0x31, 7, bytes(range(256)))]:
    body = bytes([ty, seq]) + p
    print((cobs_encode(body + struct.pack("<H", crc16(body))) + b"\\x00").hex())
`;
const ref = execFileSync("python3", ["-c", py], { encoding: "utf8" }).trim().split("\n");
const mine = [
  F.frame(0x01, 1, [2, 0, 0, 0, 0, 0, 0, 0, 0, ...Buffer.from("webble")]),
  F.frame(0x30, 200, [0x80, 2]),
  F.frame(0x31, 7, Array.from({ length: 256 }, (_, i) => i)),
].map((u) => Buffer.from(u).toString("hex"));
assert.deepEqual(mine, ref);
ok("frames match tools/ferrite_link.py byte for byte");

{
  const d = new F.Deframer();
  const a = F.frame(0xB3, 9, [0, 0, ...Buffer.from("screen Home\n")]);
  const b = F.frame(0x81, 10, [0, 2, 240, 1]);
  const wire = [...Buffer.from("junk text\r\n"), 0, ...a, ...b];
  const got = [];
  for (let i = 0; i < wire.length; i += 7) got.push(...d.push(Uint8Array.from(wire.slice(i, i + 7))));
  assert.equal(got.length, 2);
  assert.equal(got[0].ty, 0xB3);
  assert.equal(got[1].seq, 10);
  assert.equal(d.junk, 1, "the terminal text isn't a frame");
  const bad = Array.from(F.frame(0x81, 1, [0]));
  bad[2] ^= 0x40;
  assert.equal(new F.Deframer().push(Uint8Array.from(bad)).length, 0, "a bad CRC is dropped");
}
ok("deframer: any chunking, junk and bad CRCs dropped");

{
  const ui = F.parseUi('screen Quick "Quick" title="Quick"\nbar lock=0 battery=82 clock=0:00\nrow 1 "Squelch" "4" sel\nrow 2 "TX power" "Low"\nrow 3 "Mode" "FM" dot\nhints "^v" "Move" "BACK" "Back"\ntoast "Saved" "ch 12"\n');
  assert.equal(ui.screen.id, "Quick");
  assert.equal(ui.bar.battery, 82);
  assert.equal(ui.rows.length, 3);
  assert.deepEqual([ui.sel.num, ui.sel.name, ui.sel.value], [1, "Squelch", "4"]);
  assert.ok(ui.rows[2].dot);
  assert.deepEqual(ui.hints, [["^v", "Move"], ["BACK", "Back"]]);
  assert.deepEqual(ui.toast, { head: "Saved", body: "ch 12" });
  const [k, pos, kv] = F.parseUiLine('card A active=1 head="VFO \\"x\\"" freq=146.52000');
  assert.deepEqual([k, pos, kv.active, kv.head, kv.freq], ["card", ["A"], 1, 'VFO "x"', "146.52000"]);
}
ok("parseUi as tools/ferrite_link.py parse_ui");

// ---- TERM-2: a 0x00 sync before a frame sent after 5 s of quiet (as tools/ferrite_link.py) ----
{
  const sent = [];
  const link = new F.Link({ write: async (b) => { sent.push([...b]); } });
  let now = 1000;
  link.now = () => now;
  await link.sync();
  sent.length = 0;
  const f1 = [...F.frame(0x30, 1, [1, 2])];
  link.send(0x30, [1, 2], 10).catch(() => {});
  now += F.QUIET_MS - 1;
  link.send(0x30, [1, 2], 10).catch(() => {});
  now += F.QUIET_MS;
  link.send(0x30, [1, 2], 10).catch(() => {});
  await new Promise((r) => setTimeout(r, 30));
  assert.deepEqual(sent[0], f1, "right after the sync: no extra 0x00");
  assert.equal(sent[1][0], f1[0], "within 5 s of the last frame: none");
  assert.equal(sent[2][0], 0, "after 5 s of quiet: a 0x00 first");
  assert.deepEqual(sent[2].slice(1), [...F.frame(0x30, 3, [1, 2])]);
  link.close();
}
ok("a 0x00 sync before a frame after 5 s of quiet (TERM-2)");

// ---- The fake radio: the page's client through 20-byte chunks both ways ----
async function client(transport) {
  const link = new F.Link(transport);
  const radio = new F.Radio(link);
  await link.sync();
  return { link, radio };
}
{
  const t = new F.FakeTransport();
  const { radio } = await client(t);
  const h = await radio.hello();
  assert.deepEqual([h.protocol, h.maxPayload, h.mayGo, h.version, h.build, h.bench], [2, 240, true, "fake 0.1", "tx-dry", true]);
  const info = await radio.info();
  assert.deepEqual([info.channels, info.zones, info.presets, info.settingsLen, info.callsign, info.build], [21, 3, 2, 300, "N0CALL", "tx-dry"]);
  let ui = await radio.ui();
  assert.equal(ui.screen.id, "Home");
  assert.equal(ui.cards.length, 3);
  await radio.keys("MENU DOWN DOWN");
  ui = await radio.ui();
  assert.equal(ui.screen.id, "Quick");
  assert.equal(ui.sel.name, "Mode", "two pages through MORE");
  await radio.keys("UP UP");
  await radio.knob(-3);
  assert.equal((await radio.ui()).sel.value, "1");
  assert.equal(await radio.cmd("get 0 9 2 1"), "fake radio: get 0 9 2 1\n");
  assert.ok(t.writes.every((n) => n <= F.CHUNK), "every write within the 20-byte ATT payload");
  const on = await radio.pttOn();
  assert.equal(on.status, 0);
  assert.equal((await radio.pttAlive()).status, 0);
  assert.equal((await radio.pttOff()).status, 0);
  assert.equal((await radio.pttAlive()).status, 9, "not keyed after off");
  await assert.rejects(radio.press("NOPE"), /no key/);
}
ok("fake radio: HELLO, INFO, UI with MORE, keys, knob, CMD, PTT, chunked writes");
{
  const { radio } = await client(new F.FakeTransport({ pin: true }));
  assert.equal((await radio.hello()).mayGo, false);
  await assert.rejects(radio.info(), (e) => e.status === 5);
  await assert.rejects(radio.auth("1111"), (e) => e.status === 4);
  await radio.auth("1234");
  assert.equal((await radio.info()).callsign, "N0CALL");
}
ok("fake radio: the PIN lock (LINK-30)");

// ---- The Link panel (BLE MTU test): the meter, big readback and write test against the fake radio ----
{
  const t = new F.FakeTransport();
  const { link, radio } = await client(t);
  await radio.hello();
  const b = await radio.bigReadback();
  assert.ok(b.ok && !b.error && b.junk === 0, JSON.stringify(b));
  assert.ok(b.notes > 10 && b.largest === 20 && b.bytes > 500, "MTU 23: 20-byte notifications only");
  const m = link.meter.summary();
  assert.deepEqual([m.largest, m.mtu, m.hist[0][1] === m.count], [20, 23, true]);
  assert.match(m.mtuText, /^at least 23 \(only/);
  assert.ok(m.gaps > 0 && m.gapMin >= 0 && m.gapMedian >= m.gapMin && m.bps > 0 && m.nps > 0);
  const w = await radio.writeTest();
  assert.deepEqual(w.map((r) => [r.size, r.ok]), [[20, true], [100, false], [244, false]]);
  assert.equal((await radio.hello()).protocol, 2, "the link still works after the zeros");
  link.meter.reset();
  assert.equal(link.meter.summary().count, 0);
}
ok("link panel, fake MTU 23: 20-byte notifications, MTU at least 23, writes over 20 refused");
{
  const t = new F.FakeTransport({ mtu: 247 });
  const { link, radio } = await client(t);
  assert.equal(await t.connect(), "fake radio (MTU 247)");
  await radio.hello();
  const b = await radio.bigReadback();
  assert.ok(b.ok, JSON.stringify(b));
  assert.ok(b.largest > 100 && b.largest <= 244, `largest ${b.largest}`);
  assert.equal(b.notes, b.frames, "one notification per reply frame");
  const m = link.meter.summary();
  assert.equal(m.mtu, m.largest + 3);
  assert.equal(m.mtuText, `at least ${m.largest + 3}`);
  assert.ok(m.hist[2][1] >= 1 && m.hist[3][1] === 0, JSON.stringify(m.hist));
  const w = await radio.writeTest([20, 100, 244, 245]);
  assert.deepEqual(w.map((r) => [r.size, r.ok]), [[20, true], [100, true], [244, true], [245, false]]);
  assert.match(w[3].error, /MTU-3/);
  assert.equal((await radio.info()).callsign, "N0CALL", "zeros are only separators to the radio");
  assert.ok(t.writes.filter((n) => n > F.CHUNK).every((n) => n === 100 || n === 244), "only the raw test writes exceed 20");
}
ok("link panel, fake MTU 247: big notifications, inferred MTU, write sizes up to 244 taken");
{
  // The page script (not the core) at least compiles.
  const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((x) => x[1]);
  assert.equal(scripts.length, 2);
  new Function(scripts[1]);
  assert.match(html, /bt pace 0 0/);
  assert.match(html, /adb reverse tcp:8000 tcp:8000/);
  assert.ok(!/FF31\b[^\n]*getCharacteristic|0xFF31|getCharacteristic\((?!BLE_DATA)/.test(html), "only FFE1 is asked for");
}
ok("page script compiles; the panel's how-to text is there; only FFE1 is used");

// ---- The simulator, through 20-byte paced chunks ----
const simArg = process.argv.indexOf("--sim");
if (simArg > 0) {
  const [, host, port] = process.argv[simArg + 1].split(":");
  const sock = createConnection({ host, port: Number(port) });
  await new Promise((r, j) => { sock.once("connect", r); sock.once("error", j); });
  const tcp = {
    onData: null,
    writes: [],
    async write(bytes) {
      for (let i = 0; i < bytes.length; i += F.CHUNK) {
        const part = bytes.slice(i, i + F.CHUNK);
        this.writes.push(part.length);
        sock.write(part);
        await F.sleep(F.PACE_MS);
      }
    },
  };
  sock.on("data", (b) => tcp.onData && tcp.onData(new Uint8Array(b)));
  const { radio } = await client(tcp);
  const h = await radio.hello("webble test");
  assert.equal(h.protocol, 2);
  const info = await radio.info();
  assert.ok(info.channels >= 0 && info.version.length > 0);
  await radio.press("BACK", "long");
  let ui = await radio.ui();
  assert.equal(ui.screen.id, "Home", JSON.stringify(ui.screen));
  await radio.keys("MENU");
  ui = await radio.ui();
  assert.equal(ui.screen.id, "Quick");
  assert.ok(ui.rows.length >= 5 && ui.sel, "rows and a selection");
  await radio.press("BACK", "long");
  assert.equal((await radio.ui()).screen.id, "Home");
  assert.match(await radio.cmd("get 0 9 2 1"), /0 9 2 1 Bluetooth/);
  assert.match(await radio.uiText(1), /SIDE1 0 PF1/);
  assert.ok(tcp.writes.every((n) => n <= F.CHUNK));
  sock.destroy();
  ok(`simulator ${process.argv[simArg + 1]}: HELLO, INFO (${info.version}, ${info.build}), UI, keys, CMD in 20-byte chunks`);
}
console.log(`${passed} passed`);
process.exit(0);
