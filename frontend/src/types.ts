export interface Point { x:number; y:number }
export interface Pad extends Point { number:string; width:number; height:number; shape:string; rotation?:number; drill?:number; drillWidth?:number; type?:string; layer?:string; points?:Point[] }
export interface Line { x1:number; y1:number; x2:number; y2:number; width?:number; layer?:string; unit?:number }
export interface Circle extends Point { radius:number; width?:number; layer?:string; unit?:number }
export interface Polygon { points:Point[]; width?:number; layer?:string; filled?:boolean; fill?:boolean; unit?:number }
export interface FootprintText extends Point { text:string; size?:number; layer?:string; rotation?:number }
export interface Pin extends Point { number:string; name:string; length?:number; rotation?:number; electricalType?:string; graphicStyle?:string; hidden?:boolean; unit?:number }
export interface Footprint { name:string; pads:Pad[]; lines:Line[]; circles:Circle[]; polygons:Polygon[]; texts:FootprintText[]; bounds?:{x:number;y:number;width:number;height:number} }
export interface SymbolData { name?:string; pins:Pin[]; lines:Line[]; rects?:{x:number;y:number;width:number;height:number;fill?:boolean;unit?:number}[]; circles?:Circle[]; polygons?:Polygon[]; texts?:FootprintText[]; units?:number }
export interface Part { id:string; lcscId:string; name:string; description:string; manufacturer:string; package:string; source:string; warnings:string[]; footprint:Footprint; symbol:SymbolData; sourceUrl?:string; model?:{uuid:string} }
export interface Settings { outputDirectory:string; theme:'light'|'dark'; libraryName:string; includeSymbol:boolean; includeFootprint:boolean; includeModel:boolean; openAfterExport:boolean; exportTarget:string; previewContent:'footprint'|'symbol'; previewLayout:'2d'|'split'|'3d'; nativeConfig:NativeConfig; [key:string]:unknown }
export interface ExportHistoryEntry extends ExportMetadata {id:string;timestamp:string;target:string;partIds:string[];partNames:string[];partCount:number;status:'success'|'error';directory:string;files:string[];warnings:string[];error?:string}
export interface Bootstrap { version:string; settings:Settings; parts:Part[]; capabilities?:Record<string,boolean>; history?:ExportHistoryEntry[] }
export interface ExportRequest { partIds:string[]; outputDirectory:string; libraryName:string; includeSymbol:boolean; includeFootprint:boolean; target:string; includeModel?:boolean; nativeConfig?:NativeConfig }
export interface ExportResult extends ExportMetadata { directory:string; files:string[]; warnings:string[] }
export interface LogEntry { id:number; time:string; message:string; level:'info'|'success'|'error'|'warn' }
export interface UpdateInfo {currentVersion:string;latestVersion:string;available:boolean;releaseNotes:string;publishedAt:string;downloadUrl:string;releaseUrl:string;sha256:string;size:number;message?:string}
export interface UpdateDownloadResult {path:string;version:string;verified:boolean}
export interface PartSearchItem {lcscId:string;name:string;manufacturer:string;package:string;description:string;sourceUrl:string}
export interface PartSearchResult {query:string;page:number;pageSize:number;total:number;hasMore:boolean;items:PartSearchItem[];source:string;limit?:number;truncated?:boolean;message?:string}

export interface NativeConfig {enabled:boolean;libraryMode:'new'|'append';padsRoot:string;padsSymbolLibrary:string;padsFootprintLibrary:string;captureExe:string;allegroExe:string;allegroProduct:string;captureLibrary:string;allegroDirectory:string;captureUnitsPerInch:number;modelAssociationMode:'prepare'|'native';padsModelExe:string;padsReuseDirectory:string}
export type NativePathKind='padsRoot'|'padsSymbolLibrary'|'padsFootprintLibrary'|'captureExe'|'allegroExe'|'captureLibrary'|'allegroDirectory'|'padsModelExe'|'padsReuseDirectory';
export interface NativeInstallation {id:string;product:string;root:string;executable:string;version:string;architecture:string;source:string;ready:boolean;runtimeVerified:boolean;message?:string}
export interface NativeEnvironment {pads:NativeInstallation[];cadence:NativeInstallation[];warnings:string[]}
export interface ExportMetadata {artifactType?:string;nativeGenerated?:boolean;runtimeVerified?:boolean;backupDirectory?:string;validationStatus?:string;modelAssociations?:ModelAssociation[]}
export interface ModelAssociation {partId:string;entry:string;target:string;modelFile?:string;modelSha256?:string;modelName?:string;status:'prepared'|'associated'|'unavailable'|'unsupported';verified:boolean;geometryVerified?:boolean;expectedBBoxMm?:number[];actualBBoxMm?:number[];message?:string}
export interface PartListResult {fileName:string;ids:string[];duplicates:number;totalRows:number;warnings:string[]}
