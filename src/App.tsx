import { Link, Route, Routes } from "react-router";
import { scoped } from "#lib/utils/cx.js";
import ExecutionPage from "./pages/ExecutionPage.js";
import HomePage from "./pages/HomePage.js";
import TemplateEditorPage from "./pages/TemplateEditorPage.js";
import styles from "./App.module.css";

const c = scoped(styles);

export default function App() {
  return (
    <div className={c("app")}>
      <header className={c("app-header")}>
        <Link to="/" className={c("app-title")}>
          procnote
        </Link>
        <span className={c("app-subtitle")}>Procedure Execution</span>
      </header>
      <main className={c("app-main")}>
        <Routes>
          <Route path="/" element={<HomePage />} />
          <Route path="/execution/:id" element={<ExecutionPage />} />
          <Route path="/templates/edit" element={<TemplateEditorPage />} />
        </Routes>
      </main>
    </div>
  );
}
