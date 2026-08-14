import { defineStore } from "pinia";
import {
  runtimeList,
  runtimeStart,
  runtimeStop,
  runtimeStatus,
  runtimeInstall,
  siteList,
  siteAdd,
  siteUpdate,
  siteRemove,
  type Site,
  type RuntimeInfo,
} from "../api";

export const useAppStore = defineStore("app", {
  state: () => ({
    runtimes: [] as RuntimeInfo[],
    sites: [] as Site[],
    running: false,
    loading: false,
  }),

  getters: {
    defaultRuntime: (s) => s.runtimes.find((r) => r.is_default) ?? s.runtimes[0],
    hasRuntime: (s) => s.runtimes.length > 0,
  },

  actions: {
    async refreshRuntimes() {
      this.runtimes = await runtimeList();
    },

    async refreshSites() {
      this.sites = await siteList();
    },

    async refreshStatus() {
      this.running = await runtimeStatus();
    },

    async installRuntime(version: string) {
      await runtimeInstall(version);
      await this.refreshRuntimes();
    },

    async startRuntime() {
      await runtimeStart();
      this.running = true;
    },

    async stopRuntime() {
      await runtimeStop();
      this.running = false;
    },

    async reloadRuntime() {
      const { runtimeReload } = await import("../api");
      await runtimeReload();
      await this.refreshStatus();
    },

    async createSite(site: Site) {
      await siteAdd(site);
      await this.refreshSites();
    },

    async updateSite(site: Site) {
      await siteUpdate(site);
      await this.refreshSites();
    },

    async deleteSite(id: string) {
      await siteRemove(id);
      await this.refreshSites();
    },
  },
});
