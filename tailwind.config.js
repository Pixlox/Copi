/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      animation: {
        "overlay-open": "overlay-open 120ms ease-out",
        "overlay-close": "overlay-close 80ms ease-in",
      },
      keyframes: {
        "overlay-open": {
          "0%": { transform: "scale(0.96)", opacity: "0" },
          "100%": { transform: "scale(1)", opacity: "1" },
        },
        "overlay-close": {
          "0%": { transform: "scale(1)", opacity: "1" },
          "100%": { transform: "scale(0.96)", opacity: "0" },
        },
      },
    },
  },
  plugins: [],
};
