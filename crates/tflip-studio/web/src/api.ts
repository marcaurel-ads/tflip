import type { Answer } from "./bindings/Answer";
import type { ApiError } from "./bindings/ApiError";
import type { Command } from "./bindings/Command";
import type { Reply } from "./bindings/Reply";
import type { Request } from "./bindings/Request";

const TOKEN_KEY = "tflip-studio-token";

/** In the fragment, so the browser never sends it to the server; kept in
 * sessionStorage, so a reload still has it after the address bar is
 * cleared. */
function takeToken(): string | null {
  const token = new URLSearchParams(location.hash.slice(1)).get("token");
  if (token !== null) {
    sessionStorage.setItem(TOKEN_KEY, token);
    history.replaceState(null, "", location.pathname + location.search);
  }
  return sessionStorage.getItem(TOKEN_KEY);
}

const token = takeToken();

/** Thrown rather than the plain ApiError, so a view's `catch` gets an
 * `Error` (lint forbids throwing anything else) that still carries the
 * ADS code. */
export class ApiFailure extends Error {
  readonly error: ApiError;

  constructor(error: ApiError) {
    super(error.message);
    this.name = "ApiFailure";
    this.error = error;
  }
}

/** Not an ApiFailure: the fix is opening the studio's link, not the
 * input. */
export class TokenRejected extends Error {
  constructor() {
    super("Open the link tflip-studio printed when it started.");
    this.name = "TokenRejected";
  }
}

/** The stored passwords' key is not in memory: ask for the master password,
 * unlock, and try again. */
export class VaultLocked extends Error {
  constructor() {
    super("The master password is needed.");
    this.name = "VaultLocked";
  }
}

/** The engine's code for an unknown connection ID: the studio restarted, or
 * the connection was closed elsewhere. */
export const AE_INVALID_HANDLE = 5018;

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const headers: Record<string, string> = {};
  if (token !== null) {
    headers.Authorization = `Bearer ${token}`;
  }
  if (body !== undefined) {
    headers["Content-Type"] = "application/json";
  }
  const response = await fetch(path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  // Before reading the body: a rejected token and a locked vault come
  // without one.
  if (response.status === 401) {
    throw new TokenRejected();
  }
  if (response.status === 423) {
    throw new VaultLocked();
  }
  // DELETE answers 204, which has no JSON to parse.
  if (response.status === 204) {
    return undefined as T;
  }
  const answer: unknown = await response.json();
  if (!response.ok) {
    throw new ApiFailure(answer as ApiError);
  }
  return answer as T;
}

export function get<T>(path: string): Promise<T> {
  return request("GET", path);
}

export function post<T>(path: string, body: unknown): Promise<T> {
  return request("POST", path, body);
}

export function del(path: string): Promise<void> {
  return request("DELETE", path);
}

/** Both error classes carry text meant for the user. Anything else is a
 * bug, but the page still says something. */
export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The browser does not say why a socket failed or closed: a wrong token,
 * a foreign Origin and a stopped studio all look alike. */
export class SocketClosed extends Error {
  constructor() {
    super("The connection to tflip-studio was lost. Reload the page.");
    this.name = "SocketClosed";
  }
}

type Waiting = {
  resolve: (answer: Answer) => void;
  reject: (error: Error) => void;
};

/** One per page: the server ties the page's cursors to it, so closing it
 * closes them. */
export class SqlSocket {
  private readonly socket: WebSocket;
  private readonly opened: Promise<void>;
  private readonly waiting = new Map<number, Waiting>();
  private next = 0;

  constructor(connection: string) {
    const url = new URL(`/api/connections/${connection}/socket`, location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    if (token !== null) {
      url.searchParams.set("token", token);
    }
    this.socket = new WebSocket(url);
    this.opened = new Promise((resolve, reject) => {
      this.socket.addEventListener("open", () => resolve());
      this.socket.addEventListener("error", () => reject(new SocketClosed()));
    });
    // Each request awaits it and reports the failure; this only keeps an
    // unused socket's failure from counting as unhandled.
    this.opened.catch(() => undefined);
    this.socket.addEventListener("message", (event) => {
      const { id, ...answer } = JSON.parse(event.data as string) as Reply;
      const waiting = this.waiting.get(id);
      this.waiting.delete(id);
      if (answer.type === "failed") {
        waiting?.reject(
          new ApiFailure({ code: answer.code, message: answer.message }),
        );
      } else {
        waiting?.resolve(answer);
      }
    });
    this.socket.addEventListener("close", () => {
      for (const waiting of this.waiting.values()) {
        waiting.reject(new SocketClosed());
      }
      this.waiting.clear();
    });
  }

  execute(sql: string) {
    return this.ask({ type: "execute", sql }, "cursor", "affected");
  }

  rows(cursor: number, start: number, count: number) {
    return this.ask({ type: "rows", cursor, start, count }, "rows");
  }

  closeCursor(cursor: number) {
    return this.ask({ type: "close", cursor }, "closed");
  }

  close() {
    this.socket.close();
  }

  private async ask<T extends Answer["type"]>(
    command: Command,
    ...expected: T[]
  ): Promise<Extract<Answer, { type: T }>> {
    await this.opened;
    // A closed socket drops what is sent, and the reply would never come.
    if (this.socket.readyState !== WebSocket.OPEN) {
      throw new SocketClosed();
    }
    const id = this.next++;
    const answer = await new Promise<Answer>((resolve, reject) => {
      this.waiting.set(id, { resolve, reject });
      const request: Request = { id, ...command };
      this.socket.send(JSON.stringify(request));
    });
    if (!(expected as string[]).includes(answer.type)) {
      throw new Error(`Unexpected ${answer.type} reply to ${command.type}.`);
    }
    return answer as Extract<Answer, { type: T }>;
  }
}
