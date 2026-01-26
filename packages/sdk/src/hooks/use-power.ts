import { useMutation } from "@tanstack/react-query";
import { useBridgesContext } from "../context/bridges-provider";

/**
 * Hook for power control actions (REST only).
 *
 * @example
 * ```tsx
 * const { shutdown, restart, sleep, hibernate, wakeOnLan } = usePower(bridgeId);
 *
 * // Put the system to sleep
 * sleep.mutate();
 *
 * // Restart the system
 * restart.mutate();
 *
 * // Wake up a sleeping system (requires MAC address stored on bridge)
 * wakeOnLan.mutate();
 * ```
 */
export function usePower(bridgeId: string) {
  const { bridges } = useBridgesContext();
  const bridge = bridges.get(bridgeId);

  const createPowerMutation = (endpoint: string) => {
    return useMutation({
      mutationFn: async () => {
        if (!bridge) {
          throw new Error("Bridge not found");
        }

        const { config } = bridge;
        const protocol = config.secure ? "https" : "http";
        const baseUrl = `${protocol}://${config.host}:${config.port}`;

        const headers: HeadersInit = {};
        if (config.apiKey) {
          headers.Authorization = `Bearer ${config.apiKey}`;
        }

        const response = await fetch(`${baseUrl}/api/pw/${endpoint}`, {
          method: "POST",
          headers,
        });

        if (!response.ok) {
          throw new Error(`Failed to ${endpoint}: ${response.statusText}`);
        }
      },
    });
  };

  const wakeOnLan = useMutation({
    mutationFn: async () => {
      if (!bridge) {
        throw new Error("Bridge not found");
      }

      const mac = bridge.mac;
      if (!mac) {
        throw new Error(
          "MAC address not configured for this bridge. Update bridge with mac field.",
        );
      }

      // Send WoL packet using the sendWolPacket utility
      await sendWolPacket(mac, { ip: bridge.config.host });
    },
  });

  return {
    shutdown: createPowerMutation("shutdown"),
    restart: createPowerMutation("restart"),
    sleep: createPowerMutation("sleep"),
    hibernate: createPowerMutation("hibernate"),
    wakeOnLan,
  };
}

/**
 * Send a Wake-on-LAN magic packet to wake a sleeping/off computer.
 *
 * **IMPORTANT**: This function only works in Node.js environments (Electron, server-side).
 * It will NOT work in browser environments due to UDP socket restrictions.
 *
 * For browser-only apps, you'll need a server/proxy to send the WoL packet.
 *
 * @param mac - MAC address in format "00:11:22:33:44:55"
 * @param options.ip - Optional IP to derive broadcast address from
 */
export async function sendWolPacket(
  mac: string,
  options?: { ip?: string },
): Promise<void> {
  // Check if we're in a Node.js environment
  if (typeof window !== "undefined" && !("process" in window)) {
    throw new Error(
      "Wake-on-LAN requires Node.js (dgram module). " +
        "This function does not work in browser environments. " +
        "Use a server-side proxy to send WoL packets.",
    );
  }

  // Dynamic import for Node.js dgram module
  const dgram = await import("dgram").catch(() => null);
  if (!dgram) {
    throw new Error(
      "dgram module not available. Wake-on-LAN requires Node.js environment.",
    );
  }

  const addresses = new Set(["255.255.255.255"]);
  if (options?.ip) {
    const parts = options.ip.split(".");
    if (parts.length === 4) {
      addresses.add(`${parts[0]}.${parts[1]}.${parts[2]}.255`);
    }
  }

  return new Promise((resolve, reject) => {
    // Parse MAC address
    const macBytes = mac
      .replace(/[:-]/g, "")
      .match(/.{2}/g)
      ?.map((hex) => parseInt(hex, 16));

    if (!macBytes || macBytes.length !== 6) {
      reject(new Error(`Invalid MAC address: ${mac}`));
      return;
    }

    // Create magic packet: 6x 0xFF + 16x MAC address
    const magicPacket = Buffer.alloc(102);
    for (let i = 0; i < 6; i++) magicPacket[i] = 0xff;
    for (let i = 0; i < 16; i++) {
      macBytes.forEach((byte, j) => {
        magicPacket[6 + i * 6 + j] = byte;
      });
    }

    const socket = dgram.createSocket("udp4");

    socket.once("error", (err: Error) => {
      socket.close();
      reject(err);
    });

    socket.bind(() => {
      socket.setBroadcast(true);

      const sends = Array.from(addresses).map((addr) => {
        return new Promise<void>((res) => {
          socket.send(magicPacket, 9, addr, () => {
            res();
          });
        });
      });

      Promise.all(sends).finally(() => {
        socket.close();
        resolve();
      });
    });
  });
}
