package core

import (
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Compact lexical fixture; real topology/volume validation uses independent
// OCCT integration fixtures, not this intentionally minimal entity graph.
const modelSTEPFixture = `ISO-10303-21;
HEADER;FILE_DESCRIPTION(('test; #777 fake MAPPED_ITEM'),'2;1');FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;
DATA;
/* comment; with #0 */ #1=CARTESIAN_POINT('name with ''quote;',(1.,2.,-3.));
#2=CARTESIAN_POINT('',(2.,2.,-3.));
#3=CARTESIAN_POINT('',(1.,4.,-3.));
#4=CARTESIAN_POINT('',(1.,2.,1.));
#5=DIRECTION('',(1.,0.,0.));
#6=CARTESIAN_POINT('2d unchanged',(1.,2.));
#7=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));
#8=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#102,#9),#10);
#9=MANIFOLD_SOLID_BREP('',#11);
#10=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#7)) REPRESENTATION_CONTEXT('',''));
#11=CLOSED_SHELL('',());
#12=SHAPE_DEFINITION_REPRESENTATION(#13,#8);
#13=PRODUCT_DEFINITION_SHAPE('','',#14);
#14=PRODUCT('','','',());
#100=CARTESIAN_POINT('',(0.,0.,0.));
#101=DIRECTION('',(0.,0.,1.));
#102=AXIS2_PLACEMENT_3D('',#100,#101,#5);
ENDSEC;END-ISO-10303-21;`
const modelOBJFixture = "v 1 2 -3\nv 2 2 -3\nv 1 4 -3\nv 1 2 1\n"

func TestModelSTEPZRotationOffsetAndNegativeLeads(t *testing.T) {
	model := Model{PlacementConvention: "easyeda-std-kicad-v1", X: 10, Y: -5, Z: .25, RotationZ: -90, ScaleX: 1, ScaleY: 1, ScaleZ: 1}
	got, e := PrepareModelSTEP([]byte(modelSTEPFixture), []byte(modelOBJFixture), model)
	if e != nil {
		t.Fatal(e)
	}
	if got.Points != 5 || got.Directions != 2 {
		t.Fatal(got.Points, got.Directions)
	}
	for _, s := range []string{"CARTESIAN_POINT('name with ''quote;',(8,-4,-2.75))", "DIRECTION('',(6.12323399573676e-17,1,0))", "#6=CARTESIAN_POINT('2d unchanged',(1.,2.))", "FILE_DESCRIPTION(('test; #777 fake MAPPED_ITEM')"} {
		if !strings.Contains(string(got.STEP), s) {
			t.Fatalf("missing preserved/transformed record %q", s)
		}
	}
	want := []float64{6, -4, -2.75, 8, -3, 1.25}
	for i, v := range want {
		if math.Abs(got.ExpectedBBoxMM[i]-v) > 1e-10 {
			t.Fatal(got.ExpectedBBoxMM)
		}
	}
	if !strings.Contains(string(got.STEP), "-2.75") {
		t.Fatal("negative lead was moved onto board floor")
	}
}
func TestModelSTEPRejectsUncertainTransformAndStructure(t *testing.T) {
	base := Model{PlacementConvention: "easyeda-std-kicad-v1"}
	for _, model := range []Model{{}, {PlacementConvention: "unknown"}, {Prepared: true}, {PlacementConvention: base.PlacementConvention, RotationX: 90}, {PlacementConvention: base.PlacementConvention, RotationY: 90}, {PlacementConvention: base.PlacementConvention, ScaleX: 2}, {PlacementConvention: base.PlacementConvention, X: math.NaN()}, {PlacementConvention: base.PlacementConvention, SourceWidthMM: 50}} {
		if _, e := PrepareModelSTEP([]byte(modelSTEPFixture), []byte(modelOBJFixture), model); e == nil {
			t.Fatalf("accepted uncertain transform %+v", model)
		}
	}
	for _, raw := range []string{strings.Replace(modelSTEPFixture, "#9=MANIFOLD_SOLID_BREP", "#9=MAPPED_ITEM", 1), strings.Replace(modelSTEPFixture, ".MILLI.", ".INCH.", 1), strings.Replace(modelSTEPFixture, "#13=PRODUCT_DEFINITION_SHAPE", "#13=NEXT_ASSEMBLY_USAGE_OCCURRENCE", 1), strings.Replace(modelSTEPFixture, "#5=DIRECTION('',(1.,0.,0.))", "#5=DIRECTION('',(0.,0.,0.))", 1), strings.Replace(modelSTEPFixture, "#4=CARTESIAN_POINT", "#1=CARTESIAN_POINT", 1), strings.Replace(modelSTEPFixture, "END-ISO-10303-21;", "", 1), strings.Replace(modelSTEPFixture, "AUTOMOTIVE_DESIGN", "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING", 1)} {
		if _, e := PrepareModelSTEP([]byte(raw), []byte(modelOBJFixture), base); e == nil {
			t.Fatal("accepted unsupported STEP")
		}
	}
}
func TestModelSTEPMetreUnitsTranslateWithoutRescale(t *testing.T) {
	source := strings.Replace(modelSTEPFixture, ".MILLI.", "$", 1)
	model := Model{PlacementConvention: "easyeda-std-kicad-v1", X: 1000}
	got, e := PrepareModelSTEP([]byte(source), []byte(modelOBJFixture), model)
	if e != nil {
		t.Fatal(e)
	}
	if !strings.Contains(string(got.STEP), "CARTESIAN_POINT('name with ''quote;',(2,2,-3))") {
		t.Fatal("mm translation not converted to source metres")
	}
}

func TestModelSTEPLexicalCommentsAreTransformedOrRejected(t *testing.T) {
	model := Model{PlacementConvention: "easyeda-std-kicad-v1", X: 10, RotationZ: -90}
	for _, record := range []string{"#15=/*after id*/CARTESIAN_POINT('',(1.5,3.,0.));", "#15=CARTESIAN_POINT/*after type*/('',(1.5,3.,0.));"} {
		source := strings.Replace(modelSTEPFixture, "ENDSEC;END-ISO", "\n"+record+"\nENDSEC;END-ISO", 1)
		got, e := PrepareModelSTEP([]byte(source), []byte(modelOBJFixture), model)
		if e != nil {
			t.Fatal(e)
		}
		if got.Points != 6 || !strings.Contains(string(got.STEP), "(7,1.5,0)") {
			t.Fatal("commented point escaped transform")
		}
		if !strings.Contains(string(got.STEP), "/* comment; with #0 */") {
			t.Fatal("source comment lost")
		}
	}
	for _, record := range []string{"#15=(CARTESIAN_POINT((1.,2.,3.)) GEOMETRIC_REPRESENTATION_ITEM() POINT() REPRESENTATION_ITEM(''));", "#15=AXIS2_PLACEMENT_3D('',#1,$,$);", "#15=AXIS2_PLACEMENT_3D('',#1,#5,$);", "#15=CARTESIAN_POINT('',(1./*coord*/,2.,3.));", "#15=MANIFOLD_SOLID_BREP('',#99999);"} {
		source := strings.Replace(modelSTEPFixture, "ENDSEC;END-ISO", "\n"+record+"\nENDSEC;END-ISO", 1)
		if _, e := PrepareModelSTEP([]byte(source), []byte(modelOBJFixture), model); e == nil {
			t.Fatalf("accepted %s", record)
		}
	}
	model.RotationZ = -180
	source := strings.Replace(modelSTEPFixture, "ENDSEC;END-ISO", "#15=AXIS2_PLACEMENT_3D('',#1,$,$); ENDSEC;END-ISO", 1)
	if _, e := PrepareModelSTEP([]byte(source), []byte(modelOBJFixture), model); e == nil {
		t.Fatal("180 degree default frame ignored")
	}
	model.RotationZ = 1e308
	if _, e := PrepareModelSTEP([]byte(modelSTEPFixture), []byte(modelOBJFixture), model); e == nil {
		t.Fatal("overflow angle accepted")
	}
}
func TestSourceModelMetadataKeepsLegacyAnglesAndMarksInvalid(t *testing.T) {
	attrs := object{"uuid": "abc", "c_origin": "4001,3002", "c_rotation": "0,0,90", "z": "-10", "c_width": "3", "c_height": "4"}
	got, warning := parseSourceModel(attrs, 4000, 3000)
	if warning != "" || got.PlacementConvention != "easyeda-std-kicad-v1" || got.RotationZ != -90 || got.X != .254 || got.Y != -.508 || got.Z != -2.54 {
		t.Fatalf("%+v %s", got, warning)
	}
	attrs["c_rotation"] = "0,NaN,90"
	got, warning = parseSourceModel(attrs, 4000, 3000)
	if warning == "" || got.PlacementConvention != "unsupported-source-metadata" {
		t.Fatal("invalid metadata silently defaulted")
	}
}

func TestModelSTEPRootFrameIndependentAndRejectsWrongRoot(t *testing.T) {
	model := Model{PlacementConvention: "easyeda-std-kicad-v1", RotationZ: -90, X: 2}
	got, e := PrepareModelSTEP([]byte(modelSTEPFixture), []byte(modelOBJFixture), model)
	if e != nil {
		t.Fatal(e)
	}
	if !strings.Contains(string(got.STEP), "#8=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#106,#9),#10)") || !strings.Contains(string(got.STEP), "#106=AXIS2_PLACEMENT_3D('',#103,#104,#105)") {
		t.Fatal("root frame not independently cloned")
	}
	// Shared #5 is transformed for geometry; fixed root frame uses new #105.
	if !strings.Contains(string(got.STEP), "#5=DIRECTION('',(6.12323399573676e-17,1,0))") {
		t.Fatal("shared direction incorrectly exempted")
	}
	for _, raw := range []string{strings.Replace(modelSTEPFixture, "#12=SHAPE_DEFINITION_REPRESENTATION(#13,#8)", "#12=SHAPE_DEFINITION_REPRESENTATION(#13,#9)", 1), strings.Replace(modelSTEPFixture, "#100=CARTESIAN_POINT('',(0.,0.,0.))", "#100=CARTESIAN_POINT('',(1.,0.,0.))", 1), strings.Replace(modelSTEPFixture, "#101=DIRECTION('',(0.,0.,1.))", "#101=DIRECTION('',(1.,0.,0.))", 1)} {
		if _, e := PrepareModelSTEP([]byte(raw), []byte(modelOBJFixture), model); e == nil {
			t.Fatal("unsupported root accepted")
		}
	}
}
func TestModelSTEPWhitespaceOffsetsAndOwnAsymmetricSolid(t *testing.T) {
	model := Model{PlacementConvention: "easyeda-std-kicad-v1", X: 10}
	for _, prefix := range []string{"\f#15=", "\v#15=", "\u00a0#15=", "#15=\u00a0", "#15=\f"} {
		source := strings.Replace(modelSTEPFixture, "ENDSEC;END-ISO", prefix+"CARTESIAN_POINT('',(1.5,3.,0.));\nENDSEC;END-ISO", 1)
		got, e := PrepareModelSTEP([]byte(source), []byte(modelOBJFixture), model)
		if e != nil {
			t.Fatal(e)
		}
		if !strings.Contains(string(got.STEP), "CARTESIAN_POINT('',(11.5,3,0));") {
			t.Fatal("whitespace damaged entity")
		}
		if _, e := readSTEPRecords(got.STEP); e != nil {
			t.Fatal("derived file syntax invalid", e)
		}
	}
	step, e := os.ReadFile(filepath.Join("testdata", "model_asymmetric.step"))
	if e != nil {
		t.Fatal(e)
	}
	obj, e := os.ReadFile(filepath.Join("testdata", "model_asymmetric.obj"))
	if e != nil {
		t.Fatal(e)
	}
	model.RotationZ = -37
	got, e := PrepareModelSTEP(step, obj, model)
	if e != nil {
		t.Fatal(e)
	}
	if got.Points < 50 || got.Directions < 20 {
		t.Fatal("own analytic solid was not transformed")
	}
	if _, e = readSTEPRecords(got.STEP); e != nil {
		t.Fatal(e)
	}
}
