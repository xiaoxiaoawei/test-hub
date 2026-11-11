// Package Studio's small, independently written standard COM class-factory
// bridge. It does not register a DLL, modify activation, or emulate a license.
using System;
using System.IO;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using TYPEKIND = System.Runtime.InteropServices.ComTypes.TYPEKIND;
using TYPEATTR = System.Runtime.InteropServices.ComTypes.TYPEATTR;

public static class StudioPadsFactory {
    [StructLayout(LayoutKind.Sequential)] struct BasicLimits {
        public long ProcessTime,JobTime;public uint Flags;public UIntPtr MinimumWorkingSet,MaximumWorkingSet;
        public uint ActiveProcesses;public UIntPtr Affinity;public uint Priority,Scheduling;
    }
    [StructLayout(LayoutKind.Sequential)] struct IoCounters {public ulong ReadOps,WriteOps,OtherOps,ReadBytes,WriteBytes,OtherBytes;}
    [StructLayout(LayoutKind.Sequential)] struct ExtendedLimits {public BasicLimits Basic;public IoCounters Io;public UIntPtr ProcessMemory,JobMemory,PeakProcessMemory,PeakJobMemory;}
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr security,string name);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job,int kind,IntPtr value,uint size);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job,IntPtr process);
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    static IntPtr lifetimeJob;

    // The helper owns this unnamed job handle for its entire lifetime. Killing
    // the helper on context cancellation also closes the job and its CAD child.
    public static void BindLifetime() {
        if(lifetimeJob!=IntPtr.Zero)return;
        IntPtr job=CreateJobObject(IntPtr.Zero,null);
        if(job==IntPtr.Zero)throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        IntPtr memory=IntPtr.Zero;
        try {
            var limits=new ExtendedLimits();limits.Basic.Flags=0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            int size=Marshal.SizeOf(typeof(ExtendedLimits));memory=Marshal.AllocHGlobal(size);
            Marshal.StructureToPtr(limits,memory,false);
            if(!SetInformationJobObject(job,9,memory,(uint)size) || !AssignProcessToJobObject(job,GetCurrentProcess()))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
            lifetimeJob=job;job=IntPtr.Zero;
        } finally {if(memory!=IntPtr.Zero)Marshal.FreeHGlobal(memory);if(job!=IntPtr.Zero)CloseHandle(job);}
    }
    [DllImport("oleaut32.dll", CharSet=CharSet.Unicode)]
    static extern int LoadTypeLibEx(string path, int registration, out ITypeLib library);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr LoadLibraryEx(string path, IntPtr reserved, uint flags);
    [DllImport("kernel32.dll", CharSet=CharSet.Ansi, ExactSpelling=true)]
    static extern IntPtr GetProcAddress(IntPtr module, string name);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    delegate int ClassEntry(ref Guid classId, ref Guid interfaceId, out IntPtr pointer);
    [ComImport, Guid("00000001-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IFactory {
        [PreserveSig] int CreateInstance(IntPtr outer, ref Guid interfaceId, out IntPtr instance);
        [PreserveSig] int LockServer([MarshalAs(UnmanagedType.Bool)] bool keepAlive);
    }

    public static object Open(string dll) {
        if (IntPtr.Size != 4) throw new PlatformNotSupportedException("PADS file factory requires x86.");
        dll=Path.GetFullPath(dll);
        if (!String.Equals(Path.GetFileName(dll), "PadsCLibInt.dll", StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException("Unexpected SDK factory filename.");
        ITypeLib types;
        string adjacentTypes=Path.ChangeExtension(dll,".tlb");
        Marshal.ThrowExceptionForHR(LoadTypeLibEx(File.Exists(adjacentTypes)?adjacentTypes:dll, 2, out types)); // REGKIND_NONE
        var classes=new List<Guid>();
        try {
            if (types.GetTypeInfoCount()>2048) throw new InvalidDataException("Invalid SDK type library.");
            for (int index=0;index<types.GetTypeInfoCount();index++) {
                TYPEKIND kind; types.GetTypeInfoType(index,out kind);
                if (kind!=TYPEKIND.TKIND_COCLASS) continue;
                ITypeInfo type;types.GetTypeInfo(index,out type);IntPtr attributes=IntPtr.Zero;
                try {type.GetTypeAttr(out attributes);classes.Add(((TYPEATTR)Marshal.PtrToStructure(attributes,typeof(TYPEATTR))).guid);}
                finally {if(attributes!=IntPtr.Zero)type.ReleaseTypeAttr(attributes);Marshal.ReleaseComObject(type);}
            }
        } finally {Marshal.ReleaseComObject(types);}
        if(classes.Count!=1)throw new NotSupportedException("SDK factory class is ambiguous; this installation is not adapted.");
        IntPtr module=LoadLibraryEx(dll,IntPtr.Zero,8); // LOAD_WITH_ALTERED_SEARCH_PATH
        if(module==IntPtr.Zero)throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        IntPtr entry=GetProcAddress(module,"DllGetClassObject");
        if(entry==IntPtr.Zero)throw new NotSupportedException("SDK does not expose a standard COM class factory.");
        var create=(ClassEntry)Marshal.GetDelegateForFunctionPointer(entry,typeof(ClassEntry));
        Guid clsid=classes[0],factoryId=new Guid("00000001-0000-0000-C000-000000000046");IntPtr rawFactory;
        Marshal.ThrowExceptionForHR(create(ref clsid,ref factoryId,out rawFactory));
        var factory=(IFactory)Marshal.GetObjectForIUnknown(rawFactory);Marshal.Release(rawFactory);
        try {
            Guid dispatch=new Guid("00020400-0000-0000-C000-000000000046");IntPtr instance;
            Marshal.ThrowExceptionForHR(factory.CreateInstance(IntPtr.Zero,ref dispatch,out instance));
            try {return Marshal.GetObjectForIUnknown(instance);} finally {Marshal.Release(instance);}
        } finally {Marshal.FinalReleaseComObject(factory);}
    }
}
