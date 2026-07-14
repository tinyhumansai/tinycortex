/** @type {import('next').NextConfig} */
const nextConfig = {
  // better-sqlite3 is a native addon; keep it out of the bundler and load it
  // as a normal Node require at runtime (server-only).
  serverExternalPackages: ["better-sqlite3"],
};

export default nextConfig;
