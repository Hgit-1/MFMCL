import type { Plugin } from 'vite';

export const PUBLIC_ROOTS: readonly string[];
export function isForbiddenPayloadPath(value: string): boolean;
export function approvedPublicFiles(publicDir: string): Promise<{ fileName: string; source: Uint8Array }[]>;
export function assertDistributionTree(dist: string): Promise<void>;
export function distributionPolicy(): Plugin;
export function verifyTauriDistribution(tauriDir: string, options?: {
  requireDist?: boolean;
  override?: string;
  platform?: string;
  onWatch?: (file: string) => void;
}): Promise<string>;
