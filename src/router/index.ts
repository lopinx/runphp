import { createRouter, createWebHashHistory } from "vue-router";

// 桌面壳与 Web 面板均以 hash 路由运行，避免静态托管路径问题
const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/dashboard" },
    {
      path: "/dashboard",
      name: "dashboard",
      component: () => import("../views/DashboardView.vue"),
      meta: { title: "仪表盘" },
    },
    {
      path: "/sites",
      name: "sites",
      component: () => import("../views/SitesView.vue"),
      meta: { title: "站点" },
    },
    {
      path: "/databases",
      name: "databases",
      component: () => import("../views/DatabasesView.vue"),
      meta: { title: "数据库" },
    },
    {
      path: "/ftp",
      name: "ftp",
      component: () => import("../views/FtpView.vue"),
      meta: { title: "FTP" },
    },
    {
      path: "/hosts",
      name: "hosts",
      component: () => import("../views/HostsView.vue"),
      meta: { title: "Hosts" },
    },
    {
      path: "/settings",
      name: "settings",
      component: () => import("../views/SettingsView.vue"),
      meta: { title: "设置" },
    },
    { path: "/:pathMatch(.*)*", redirect: "/dashboard" },
  ],
});

export default router;
