package service

import "packagestudio/internal/core"

func cloneModelAssociations(input []core.ModelAssociation) []core.ModelAssociation {
	if input == nil {
		return nil
	}
	out := append([]core.ModelAssociation{}, input...)
	for i := range out {
		out[i].ExpectedBBoxMM = append([]float64(nil), out[i].ExpectedBBoxMM...)
		out[i].ActualBBoxMM = append([]float64(nil), out[i].ActualBBoxMM...)
	}
	return out
}
