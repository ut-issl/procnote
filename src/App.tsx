import { Link, Route, Routes } from "react-router";
import OperatorName from "#lib/components/OperatorName.js";
import DesktopSyncStatus from "#lib/components/DesktopSyncStatus.js";
import SyncStatus from "#lib/components/SyncStatus.js";
import { isDesktop } from "#lib/platform.js";
import { scoped } from "#lib/utils/cx.js";
import DeviceConnectPage from "./pages/DeviceConnectPage.js";
import ExecutionPage from "./pages/ExecutionPage.js";
import HomePage from "./pages/HomePage.js";
import TemplatePage from "./pages/TemplatePage.js";
import UploadPage from "./pages/UploadPage.js";
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
        <OperatorName />
        {isDesktop ? <DesktopSyncStatus /> : <SyncStatus />}
      </header>
      <main className={c("app-main")}>
        <Routes>
          <Route path="/" element={<HomePage />} />
          <Route path="/execution/:id" element={<ExecutionPage />} />
          <Route path="/templates/:procedureId" element={<TemplatePage />} />
          <Route path="/upload/:token" element={<UploadPage />} />
          <Route path="/device/connect" element={<DeviceConnectPage />} />
        </Routes>
      </main>
    </div>
  );
}
