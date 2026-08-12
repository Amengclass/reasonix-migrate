import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { MigrateTab } from "@/pages/MigrateTab";
import { ExportTab } from "@/pages/ExportTab";
import { ImportTab } from "@/pages/ImportTab";
import { VerifyTab } from "@/pages/VerifyTab";

export default function App() {
  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background">
      <Tabs defaultValue="migrate" className="flex min-h-0 flex-1 flex-col">
        <TabsList className="ml-3 mt-2 w-fit">
          <TabsTrigger value="migrate">迁移</TabsTrigger>
          <TabsTrigger value="export">导出</TabsTrigger>
          <TabsTrigger value="import">导入</TabsTrigger>
          <TabsTrigger value="verify">校验</TabsTrigger>
        </TabsList>
        <TabsContent value="migrate" className="min-h-0 flex-1 pb-2 pr-2">
          <MigrateTab />
        </TabsContent>
        <TabsContent value="export" className="min-h-0 flex-1 pb-2 pr-2">
          <ExportTab />
        </TabsContent>
        <TabsContent value="import" className="min-h-0 flex-1 pb-2 pr-2">
          <ImportTab />
        </TabsContent>
        <TabsContent value="verify" className="min-h-0 flex-1 pb-2 pr-2">
          <VerifyTab />
        </TabsContent>
      </Tabs>
    </div>
  );
}
