import { createIsolation } from "jotai-scope";
import React from "react";

type TunnelEntry = {
  getChildren: () => React.ReactNode;
  id: number;
};

export type Tunnel = {
  In: React.FC<{ children: React.ReactNode }>;
  Out: React.FC;
};

type TunnelsContextValue = {
  MainMenuTunnel: Tunnel;
  WelcomeScreenMenuHintTunnel: Tunnel;
  WelcomeScreenToolbarHintTunnel: Tunnel;
  WelcomeScreenHelpHintTunnel: Tunnel;
  WelcomeScreenCenterTunnel: Tunnel;
  FooterCenterTunnel: Tunnel;
  DefaultSidebarTriggerTunnel: Tunnel;
  DefaultSidebarTabTriggersTunnel: Tunnel;
  OverwriteConfirmDialogTunnel: Tunnel;
  TTDDialogTriggerTunnel: Tunnel;
  // this can be removed once we create jotai stores per each editor
  // instance
  tunnelsJotai: ReturnType<typeof createIsolation>;
};

export const TunnelsContext = React.createContext<TunnelsContextValue>(null!);

export const useTunnels = () => React.useContext(TunnelsContext);

const tunnelsJotai = createIsolation();

const EMPTY_TUNNEL_ENTRIES: TunnelEntry[] = [];

const createTunnel = (): Tunnel => {
  let nextId = 0;
  let entries = EMPTY_TUNNEL_ENTRIES;
  const listeners = new Set<() => void>();

  const emit = () => {
    listeners.forEach((listener) => listener());
  };

  const subscribe = (listener: () => void) => {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  };

  const getSnapshot = () => entries;

  return {
    In: ({ children }) => {
      const childrenRef = React.useRef(children);
      const idRef = React.useRef<number | null>(null);
      childrenRef.current = children;

      if (idRef.current === null) {
        idRef.current = nextId++;
      }

      React.useLayoutEffect(() => {
        const id = idRef.current!;
        entries = [
          ...entries,
          {
            getChildren: () => childrenRef.current,
            id,
          },
        ];
        emit();

        return () => {
          entries = entries.filter((entry) => entry.id !== id);
          emit();
        };
      }, []);

      return null;
    },
    Out: () => {
      const currentEntries = React.useSyncExternalStore(
        subscribe,
        getSnapshot,
        () => EMPTY_TUNNEL_ENTRIES,
      );

      return React.createElement(
        React.Fragment,
        null,
        currentEntries.map((entry) =>
          React.createElement(
            React.Fragment,
            { key: entry.id },
            entry.getChildren(),
          ),
        ),
      );
    },
  };
};

export const useInitializeTunnels = () => {
  return React.useMemo((): TunnelsContextValue => {
    return {
      MainMenuTunnel: createTunnel(),
      WelcomeScreenMenuHintTunnel: createTunnel(),
      WelcomeScreenToolbarHintTunnel: createTunnel(),
      WelcomeScreenHelpHintTunnel: createTunnel(),
      WelcomeScreenCenterTunnel: createTunnel(),
      FooterCenterTunnel: createTunnel(),
      DefaultSidebarTriggerTunnel: createTunnel(),
      DefaultSidebarTabTriggersTunnel: createTunnel(),
      OverwriteConfirmDialogTunnel: createTunnel(),
      TTDDialogTriggerTunnel: createTunnel(),
      tunnelsJotai,
    };
  }, []);
};
