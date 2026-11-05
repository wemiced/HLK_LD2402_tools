import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/jetbrains-mono/500.css";
import "./styles/tokens.css";
import "./styles/app.css";
import { initLocale } from "./i18n";
import { mountShell } from "./ui/shell";

mountShell(document.getElementById("app")!);
initLocale();
