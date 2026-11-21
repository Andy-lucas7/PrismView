/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        pureBlack: "#000000",
        carbonBlack: "#0D0D0F",
        surfaceElevated: "#141418",
        mutedZinc: "#A1A1AA",
        trackZinc: "#27272A",
      },
      borderRadius: {
        panel: "26px",
        card: "16px",
      },
    },
  },
  plugins: [],
};
