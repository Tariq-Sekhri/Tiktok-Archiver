const path = require("path");

module.exports = {
  apps: [
    {
      name: "tiktok-archiver",
      script: path.join(__dirname, "target", "release", "Tiktok-Archiver.exe"),
      cwd: __dirname,
      interpreter: "none",
      autorestart: true,
      max_restarts: 30,
      min_uptime: 8000,
      exp_backoff_restart_delay: 1000,
      error_file: path.join(__dirname, "state", "pm2-error.log"),
      out_file: path.join(__dirname, "state", "pm2-out.log"),
      merge_logs: false,
      time: true,
    },
  ],
};
