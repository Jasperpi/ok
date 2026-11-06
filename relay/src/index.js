// The ok relay. A computer running ok keeps a WebSocket open to
// /r/<room>/desk; a phone opens /r/<room>/phone. Whatever one sends, the
// other gets. Every message is sealed with AES-256-GCM under a key made from
// a secret only the phone and the computer hold (it reaches the phone in
// the QR code, after the #, which browsers never send to a server), so the
// relay passes along bytes it cannot read or forge.
//
// The relay adds two plain notes of its own: it tells the computer how many
// phones are in (`!phones=N`) and tells phones whether the computer is
// (`!desk=1` or `!desk=0`). Those are the only messages starting with "!".

import { DurableObject } from "cloudflare:workers";

// A room is the first 16 bytes of a SHA-256, in hex.
const ROOM = /^\/r\/([0-9a-f]{32})\/(desk|phone)$/;
// The longest message passed on: far more than a phone sends in one go.
const LONGEST = 256 * 1024;
// Phones in one room at once.
const PHONES = 8;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const match = url.pathname.match(ROOM);
    if (match) {
      if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
        return new Response("This address takes a WebSocket.", { status: 426 });
      }
      const room = env.ROOMS.get(env.ROOMS.idFromName(match[1]));
      return room.fetch(new Request(`https://room/${match[2]}`, request));
    }
    // The phone page and its files; everything else is not here.
    if (url.protocol === "http:" && request.method === "GET") {
      url.protocol = "https:";
      return Response.redirect(url.toString(), 301);
    }
    return env.ASSETS.fetch(request);
  },
};

export class Room extends DurableObject {
  async fetch(request) {
    const role = new URL(request.url).pathname.slice(1);
    const phones = this.ctx.getWebSockets("phone");
    if (role === "phone" && phones.length >= PHONES) {
      return new Response("Too many phones.", { status: 429 });
    }
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    if (role === "desk") {
      // One computer per room: a new connection replaces an old one that
      // the network dropped without telling anyone.
      for (const old of this.ctx.getWebSockets("desk")) {
        try { old.close(4000, "replaced"); } catch {}
      }
    }
    this.ctx.acceptWebSocket(server, [role]);
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("p", "p"));
    this.tell();
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws, message) {
    if (typeof message !== "string" || message.length > LONGEST || message.startsWith("!")) return;
    const from = this.ctx.getTags(ws)[0];
    const to = from === "desk" ? "phone" : "desk";
    for (const other of this.ctx.getWebSockets(to)) {
      try { other.send(message); } catch {}
    }
  }

  async webSocketClose(ws, code) {
    try { ws.close(code === 1005 ? 1000 : code, "closing"); } catch {}
    this.tell(ws);
  }

  async webSocketError(ws) {
    this.tell(ws);
  }

  // Tells the computer how many phones are in, and the phones whether the
  // computer is. `gone` is a socket that is closing and not to be counted.
  tell(gone) {
    const open = (tag) => this.ctx.getWebSockets(tag).filter((ws) => ws !== gone && ws.readyState === WebSocket.OPEN);
    const desks = open("desk");
    const phones = open("phone");
    for (const desk of desks) {
      try { desk.send(`!phones=${phones.length}`); } catch {}
    }
    for (const phone of phones) {
      try { phone.send(`!desk=${desks.length > 0 ? 1 : 0}`); } catch {}
    }
  }
}
