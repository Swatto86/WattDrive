/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,js}"],
  theme: { extend: {} },
  plugins: [require("daisyui")],
  daisyui: {
    // `business` = dark, `corporate` = light — same pair as WattMail, so the
    // two apps look like siblings on the same desktop.
    themes: ["business", "corporate"],
    darkTheme: "business",
  },
};
