declare module "@tauri-apps/api/core" {
  export const invoke: <T>(
    command: string,
    args?: Record<string, unknown>,
  ) => Promise<T>;
}

declare module "@tauri-apps/api/event" {
  export const listen: <T>(
    event: string,
    handler: (event: { payload: T }) => void,
  ) => Promise<() => void>;
}
